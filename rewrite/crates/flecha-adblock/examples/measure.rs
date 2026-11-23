//! Times a cold load (parse the cached lists) or a warm load (deserialize `engine.dat`) in its
//! own process, and prints the process's peak resident memory: `measure cold|warm <dir>`.
//! Run `cold` first: it writes the `engine.dat` that `warm` reads.

use std::{fs, time::Instant};

use flecha_adblock::{
    AdBlocker, LISTS, Request, RequestType,
    engine::{build_engine, engine_key, save_engine},
    lists::checked_text,
    warm_start,
};

/// VmHWM from /proc/self/status: the peak resident set size, Linux only.
fn peak_rss() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .map(str::to_owned)
        })
        .map_or_else(
            || "VmHWM: unavailable".to_owned(),
            |line| line.split_whitespace().collect::<Vec<_>>().join(" "),
        )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mode, dir) = (args.next().expect("cold|warm"), args.next().expect("dir"));
    let dir = std::path::Path::new(&dir);
    let (blocker, _) = AdBlocker::new(true, &[]);
    let elapsed = match mode.as_str() {
        "cold" => {
            let texts: Vec<_> = LISTS
                .iter()
                .map(|list| {
                    let bytes = fs::read(dir.join(format!("{}.txt", list.id))).unwrap();
                    (list.id, checked_text(list.id, &bytes, false).unwrap())
                })
                .collect();
            let start = Instant::now();
            let (engine, skipped) =
                build_engine(texts.iter().map(|(id, text)| (*id, text.as_str())));
            let elapsed = start.elapsed();
            let key = engine_key(texts.iter().map(|(id, text)| (*id, text.as_str())));
            save_engine(dir, &key, &engine).unwrap();
            println!("skipped lines: {}", skipped.len());
            let mut by_error = std::collections::BTreeMap::<&str, usize>::new();
            for line in &skipped {
                *by_error.entry(line.error.as_str()).or_default() += 1;
            }
            for (error, count) in by_error {
                println!("  {count} {error}");
            }
            drop(engine);
            elapsed
        }
        "warm" => {
            let start = Instant::now();
            warm_start(&blocker, dir).unwrap();
            start.elapsed()
        }
        _ => panic!("cold|warm"),
    };
    if mode == "warm" {
        let ad = Request {
            url: "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
            source_url: "https://news.example/",
            top_level_url: "https://news.example/",
            frame_parent_url: None,
            request_type: RequestType::Script,
        };
        println!("blocks doubleclick gpt.js: {}", blocker.should_block(&ad));
        // EasyList's `@@||optout.networkadvertising.org^$document` exempts the whole page.
        let on_exempt_page = Request {
            source_url: "https://optout.networkadvertising.org/",
            top_level_url: "https://optout.networkadvertising.org/",
            ..ad
        };
        println!(
            "blocks doubleclick gpt.js on optout.networkadvertising.org: {}",
            blocker.should_block(&on_exempt_page)
        );
    }
    println!(
        "{mode}: {:.1} ms; {}",
        elapsed.as_secs_f64() * 1000.0,
        peak_rss()
    );
    let dat = fs::metadata(dir.join("engine.dat"))
        .map(|m| m.len())
        .unwrap_or_default();
    println!("engine.dat: {dat} bytes");
}
