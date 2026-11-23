//! Downloads the real lists into a directory with the real transport: `fetch_lists <dir>`.
//! Run by `scripts/check-live-lists.sh`; tests never touch the network.

use flecha_adblock::{UreqDownload, fetch::fetch_lists};

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .expect("usage: fetch_lists <dir>");
    let report = fetch_lists(dir.as_ref(), true, &UreqDownload::default());
    for warning in &report.warnings {
        eprintln!("{warning}");
    }
    match report.lists {
        Ok(lists) => {
            for list in lists {
                println!("{}: {:?}, {} bytes", list.id, list.source, list.text.len());
            }
        }
        Err(failures) => {
            for failure in failures {
                eprintln!("{failure}");
            }
            std::process::exit(1);
        }
    }
}
