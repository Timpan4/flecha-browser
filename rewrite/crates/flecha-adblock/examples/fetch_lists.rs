//! Downloads the real lists into a directory with the real transport: `fetch_lists <dir>`.
//! Run by `scripts/check-live-lists.sh`; tests never touch the network.

use flecha_adblock::{
    UreqDownload,
    fetch::{ListSource, fetch_lists},
};

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
            for list in &lists {
                println!("{}: {:?}, {} bytes", list.id, list.source, list.text.len());
            }
            // A cached copy means the real transport wasn't proven, so the live check fails.
            if lists
                .iter()
                .any(|list| list.source != ListSource::Downloaded)
            {
                eprintln!("not every list was downloaded");
                std::process::exit(1);
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
