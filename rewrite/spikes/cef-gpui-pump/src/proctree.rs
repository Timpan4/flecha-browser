//! Process discovery for the done checks: every process's stat under a
//! `/proc`-like dir, and the tree below a root. Discovery fails closed: a
//! listing that cannot be read, an entry whose stat cannot be read for a
//! reason other than the process being gone, a stat that does not parse, or a
//! missing root makes the snapshot inconclusive, and every verdict built on an
//! inconclusive snapshot fails.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

/// The fields of `/proc/PID/stat` the checks use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: i32,
    pub ppid: i32,
    pub utime: u64,
    pub stime: u64,
    /// Field 22, in clock ticks after boot. With the PID it identifies a
    /// process even after the PID is reused.
    pub start_time: u64,
}

/// Parses `/proc/PID/stat`. `comm` (field 2) may hold spaces and parentheses,
/// so fields resume after the last `)`.
pub fn parse_stat(pid: i32, stat: &str) -> Option<ProcStat> {
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let fields: Vec<&str> = rest.split(' ').collect();
    // `rest` starts at field 3 (state), so field N is fields[N - 3].
    Some(ProcStat {
        pid,
        ppid: fields.get(1)?.parse().ok()?,
        utime: fields.get(11)?.parse().ok()?,
        stime: fields.get(12)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

/// Whether a read error under `/proc/PID` means the process is gone.
fn gone(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
}

/// Every process listed under `proc_dir`, and every reason the listing is not
/// complete. A process that exits between the listing and its stat read is
/// left out without an error; nothing else is.
#[derive(Debug, Default)]
pub struct Listing {
    pub stats: Vec<ProcStat>,
    pub errors: Vec<String>,
}

pub fn list(proc_dir: &Path) -> Listing {
    let mut out = Listing::default();
    let entries = match fs::read_dir(proc_dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.errors.push(format!("list {}: {e}", proc_dir.display()));
            return out;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                out.errors.push(format!("list {}: {e}", proc_dir.display()));
                continue;
            }
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let file = entry.path().join("stat");
        match fs::read_to_string(&file) {
            Ok(stat) => match parse_stat(pid, &stat) {
                Some(s) => out.stats.push(s),
                None => out.errors.push(format!(
                    "{}: unparsed {:?}",
                    file.display(),
                    stat.trim_end()
                )),
            },
            Err(e) if gone(&e) => {}
            Err(e) => out.errors.push(format!("{}: {e}", file.display())),
        }
    }
    out
}

/// `root` and every process descended from it, root first, each after its
/// parent. Errors, root missing included, make the tree inconclusive.
#[derive(Debug, Default)]
pub struct Tree {
    pub stats: Vec<ProcStat>,
    pub errors: Vec<String>,
}

impl Tree {
    pub fn complete(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn tree(listing: Listing, root: i32) -> Tree {
    let Listing { stats, mut errors } = listing;
    let Some(root_stat) = stats.iter().find(|s| s.pid == root).copied() else {
        errors.push(format!("root {root} is not in the process listing"));
        return Tree {
            stats: Vec::new(),
            errors,
        };
    };
    let mut children: HashMap<i32, Vec<ProcStat>> = HashMap::new();
    for s in &stats {
        if s.pid != root {
            children.entry(s.ppid).or_default().push(*s);
        }
    }
    let mut out = vec![root_stat];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = children.remove(&out[i].pid) {
            out.extend(kids);
        }
        i += 1;
    }
    Tree { stats: out, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn stat_line(pid: i32, comm: &str, ppid: i32, start: u64) -> String {
        format!(
            "{pid} ({comm}) S {ppid} {pid} {pid} 0 -1 4194560 0 0 0 0 7 3 0 0 20 0 1 0 {start} 0 0\n"
        )
    }

    /// A fake `/proc` in the temp dir, named by test, PID and clock, removed on drop.
    struct FakeProc(PathBuf);
    impl FakeProc {
        fn new(test: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("proctree-{test}-{}-{nanos}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            FakeProc(dir)
        }
        fn add(&self, pid: i32, stat: &str) {
            fs::create_dir_all(self.0.join(pid.to_string())).unwrap();
            fs::write(self.0.join(pid.to_string()).join("stat"), stat).unwrap();
        }
    }
    impl Drop for FakeProc {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_complete_listing_gives_the_tree_below_the_root() {
        let p = FakeProc::new("complete");
        p.add(10, &stat_line(10, "app (x)", 1, 100));
        p.add(11, &stat_line(11, "zygote", 10, 101));
        p.add(12, &stat_line(12, "renderer", 11, 102));
        p.add(20, &stat_line(20, "other", 1, 103));
        fs::create_dir_all(p.0.join("self")).unwrap();
        let t = tree(list(&p.0), 10);
        assert!(t.complete(), "{:?}", t.errors);
        let pids: Vec<i32> = t.stats.iter().map(|s| s.pid).collect();
        assert_eq!(pids, [10, 11, 12]);
        assert_eq!(t.stats[0].start_time, 100);
        assert_eq!((t.stats[0].utime, t.stats[0].stime), (7, 3));
    }

    #[test]
    fn a_missing_root_is_inconclusive() {
        let p = FakeProc::new("no-root");
        p.add(11, &stat_line(11, "zygote", 10, 101));
        let t = tree(list(&p.0), 10);
        assert!(!t.complete());
        assert!(t.stats.is_empty());
        assert!(t.errors[0].contains("root 10"), "{:?}", t.errors);
    }

    #[test]
    fn an_unreadable_entry_is_inconclusive() {
        let p = FakeProc::new("unreadable");
        p.add(10, &stat_line(10, "app", 1, 100));
        // A stat that is a directory fails to read with EISDIR whatever the
        // test's privileges, as an entry denied by permissions would.
        fs::create_dir_all(p.0.join("11").join("stat")).unwrap();
        let t = tree(list(&p.0), 10);
        assert!(!t.complete());
        assert!(
            t.errors.iter().any(|e| e.contains("11/stat")),
            "{:?}",
            t.errors
        );
    }

    #[test]
    fn an_unparseable_stat_is_inconclusive() {
        let p = FakeProc::new("unparsed");
        p.add(10, &stat_line(10, "app", 1, 100));
        p.add(11, "11 (zygote) S 10");
        let t = tree(list(&p.0), 10);
        assert!(!t.complete());
        assert!(
            t.errors.iter().any(|e| e.contains("unparsed")),
            "{:?}",
            t.errors
        );
        // The root's own stat failing to parse leaves no root either.
        let p = FakeProc::new("unparsed-root");
        p.add(10, "garbage");
        let t = tree(list(&p.0), 10);
        assert_eq!(t.errors.len(), 2, "{:?}", t.errors);
    }

    #[test]
    fn a_process_gone_before_its_stat_read_is_not_an_error() {
        let p = FakeProc::new("gone");
        p.add(10, &stat_line(10, "app", 1, 100));
        // Listed but its stat is already gone, as for a process that exited.
        fs::create_dir_all(p.0.join("11")).unwrap();
        let t = tree(list(&p.0), 10);
        assert!(t.complete(), "{:?}", t.errors);
        assert_eq!(t.stats.len(), 1);
    }

    #[test]
    fn an_unlistable_proc_dir_is_inconclusive() {
        let p = FakeProc::new("unlistable");
        let t = tree(list(&p.0.join("missing")), 10);
        assert!(
            t.errors.len() == 2 && t.errors[0].starts_with("list "),
            "{:?}",
            t.errors
        );
    }

    #[test]
    fn the_live_proc_has_this_process_as_a_complete_root() {
        let me = std::process::id() as i32;
        let t = tree(list(Path::new("/proc")), me);
        assert!(t.complete(), "{:?}", t.errors);
        assert_eq!(t.stats[0].pid, me);
    }
}
