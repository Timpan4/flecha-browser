//! Finds and kills the processes of one runner run that carry its marker, the
//! run's own `XDG_RUNTIME_DIR=` entry, which a process keeps when it leaves the
//! run's process group or is reparented. `scripts/run-checks.sh` runs this
//! through the app binary's `--pump-sweep=` and `--pump-sweep-kill=` modes,
//! because Bash cannot open a pidfd.
//!
//! A PID read from `/proc` may be reused before it is signaled. So the kill
//! opens a pidfd first, which pins that process, then re-reads its start time
//! and environment, and signals through the same pidfd only if both still
//! match the scan. A process that exits and is replaced fails the check, and a
//! signal through a pidfd never reaches another process.
//!
//! Only processes of this user whose `environ` is readable are seen: Linux
//! hides `environ` of a non-dumpable process, so this sweep is not complete.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;

/// The fields of `/proc/PID/stat` the sweep uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub state: char,
    pub pgid: i32,
    /// Field 22, in clock ticks after boot. Two processes with one PID never
    /// share it, since the second starts after the first has exited.
    pub start_time: u64,
}

/// Parses `/proc/PID/stat`. `comm` (field 2) may hold spaces and parentheses,
/// so fields resume after the last `)`.
pub fn parse_stat(stat: &str) -> Option<Stat> {
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let fields: Vec<&str> = rest.split(' ').collect();
    // `rest` starts at field 3 (state), so field N is fields[N - 3].
    let mut state = fields.first()?.chars();
    let (Some(state), None) = (state.next(), state.next()) else {
        return None;
    };
    Some(Stat {
        state,
        pgid: fields.get(2)?.parse().ok()?,
        start_time: fields.get(19)?.parse().ok()?,
    })
}

/// Whether a NUL-separated environment block holds exactly `entry`.
pub fn has_entry(environ: &[u8], entry: &str) -> bool {
    environ.split(|b| *b == 0).any(|e| e == entry.as_bytes())
}

/// What a re-read of a scanned process shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// The same process, still marked and not a zombie: signal it.
    Same,
    /// Its stat or environment is gone: it exited.
    Gone,
    /// It exited and awaits its reaper.
    Zombie,
    /// The PID now belongs to a process that started at `now`.
    StartTimeChanged { now: u64 },
    /// The PID's process no longer carries the marker.
    MarkerGone,
    /// Its stat could not be parsed.
    Unparsed,
}

/// Decides whether the process scanned with `start_time` is still the one at
/// its PID, from a fresh read of its stat and environment (`None`: gone).
pub fn check(start_time: u64, stat: Option<&str>, environ: Option<&[u8]>, marker: &str) -> Check {
    let Some(stat) = stat else {
        return Check::Gone;
    };
    let Some(now) = parse_stat(stat) else {
        return Check::Unparsed;
    };
    if now.start_time != start_time {
        return Check::StartTimeChanged {
            now: now.start_time,
        };
    }
    if now.state == 'Z' {
        return Check::Zombie;
    }
    match environ {
        None => Check::Gone,
        Some(env) if has_entry(env, marker) => Check::Same,
        Some(_) => Check::MarkerGone,
    }
}

/// A live process of this user that carried the marker when scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    pub pid: i32,
    pub pgid: i32,
    pub state: char,
    pub start_time: u64,
    pub args: String,
}

/// The scan result: marked processes, and this user's processes whose
/// environment could not be read (a non-dumpable process hides it).
#[derive(Debug, Default)]
pub struct Scan {
    pub marked: Vec<Marked>,
    pub unreadable: Vec<i32>,
}

/// A read of a file under `/proc/PID`: `Ok(None)` when the process is gone.
fn read_gone_ok(file: String) -> io::Result<Option<Vec<u8>>> {
    match fs::read(file) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH) => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Lists this user's live processes whose environment holds `marker`, except
/// this process. A failure to list `/proc` or to read a process that still
/// exists is an error, never an empty result; an unreadable environment is
/// counted in `unreadable`.
pub fn scan(marker: &str) -> io::Result<Scan> {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let me = std::process::id() as i32;
    let mut out = Scan::default();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        match entry.metadata() {
            Ok(m) if m.uid() == uid => {}
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
        let Some(stat) = read_gone_ok(format!("/proc/{pid}/stat"))? else {
            continue;
        };
        let stat = String::from_utf8_lossy(&stat);
        let Some(st) = parse_stat(&stat) else {
            return Err(io::Error::other(format!(
                "unparsed /proc/{pid}/stat: {stat}"
            )));
        };
        if st.state == 'Z' {
            continue;
        }
        let environ = match read_gone_ok(format!("/proc/{pid}/environ")) {
            Ok(Some(env)) => env,
            Ok(None) => continue,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                out.unreadable.push(pid);
                continue;
            }
            Err(e) => return Err(e),
        };
        if !has_entry(&environ, marker) {
            continue;
        }
        let args = read_gone_ok(format!("/proc/{pid}/cmdline"))?
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        out.marked.push(Marked {
            pid,
            pgid: st.pgid,
            state: st.state,
            start_time: st.start_time,
            args,
        });
    }
    Ok(out)
}

/// What the kill did to one scanned process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Killed,
    /// Not signaled: the process at the PID failed the re-check.
    Skipped(Check),
    /// The process was gone before its pidfd could be opened.
    GoneBeforeOpen,
    /// pidfd_open or pidfd_send_signal failed for another reason, which says
    /// nothing about the process.
    Failed(String),
}

/// Sends SIGKILL to each scanned process that is still the same process and
/// still marked, through a pidfd opened before the re-check.
pub fn kill(marked: &[Marked], marker: &str) -> Vec<(i32, Outcome)> {
    marked
        .iter()
        .map(|m| (m.pid, kill_one(m.pid, m.start_time, marker)))
        .collect()
}

fn kill_one(pid: i32, start_time: u64, marker: &str) -> Outcome {
    // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
    if fd < 0 {
        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(libc::ESRCH) {
            Outcome::GoneBeforeOpen
        } else {
            Outcome::Failed(format!("pidfd_open: {err}"))
        };
    }
    let reread = || -> io::Result<Check> {
        let stat = read_gone_ok(format!("/proc/{pid}/stat"))?;
        let environ = read_gone_ok(format!("/proc/{pid}/environ"))?;
        let stat = stat.map(|s| String::from_utf8_lossy(&s).into_owned());
        Ok(check(
            start_time,
            stat.as_deref(),
            environ.as_deref(),
            marker,
        ))
    };
    let outcome = match reread() {
        Ok(Check::Same) => {
            // SAFETY: fd is a pidfd from above; a null siginfo sends SIGKILL as
            // kill(2) would.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd,
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            if rc == 0 {
                Outcome::Killed
            } else {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    Outcome::Skipped(Check::Gone)
                } else {
                    Outcome::Failed(format!("pidfd_send_signal: {err}"))
                }
            }
        }
        Ok(other) => Outcome::Skipped(other),
        Err(e) => Outcome::Failed(format!("re-read: {e}")),
    };
    // SAFETY: fd came from pidfd_open above.
    unsafe { libc::close(fd) };
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    fn stat_line(comm: &str, state: char, pgid: i32, start: u64) -> String {
        // Fields 1 to 22 of a real stat line, with ppid 1 and zeros between.
        format!(
            "4242 ({comm}) {state} 1 {pgid} {pgid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0"
        )
    }

    #[test]
    fn stat_fields_survive_odd_comm() {
        let s = parse_stat(&stat_line("a) b (c", 'S', 77, 123456)).unwrap();
        assert_eq!(
            s,
            Stat {
                state: 'S',
                pgid: 77,
                start_time: 123456
            }
        );
        assert_eq!(parse_stat("4242 (x) S 1"), None);
    }

    #[test]
    fn marker_matches_whole_entries_only() {
        let env = b"A=1\0XDG_RUNTIME_DIR=/tmp/r/runtime\0B=2\0";
        assert!(has_entry(env, "XDG_RUNTIME_DIR=/tmp/r/runtime"));
        assert!(!has_entry(env, "XDG_RUNTIME_DIR=/tmp/r"));
        assert!(!has_entry(
            b"XDG_RUNTIME_DIR=/tmp/r/runtime2\0",
            "XDG_RUNTIME_DIR=/tmp/r/runtime"
        ));
    }

    #[test]
    fn a_pid_whose_start_time_changed_is_skipped() {
        let marker = "XDG_RUNTIME_DIR=/tmp/r/runtime";
        let env = format!("{marker}\0").into_bytes();
        let same = stat_line("sleep", 'S', 10, 500);
        assert_eq!(check(500, Some(&same), Some(&env), marker), Check::Same);
        // The PID was reused by a process that started later, marker or not.
        let reused = stat_line("sleep", 'S', 10, 501);
        assert_eq!(
            check(500, Some(&reused), Some(&env), marker),
            Check::StartTimeChanged { now: 501 }
        );
        assert_eq!(
            check(500, Some(&same), Some(b"A=1\0"), marker),
            Check::MarkerGone
        );
        assert_eq!(check(500, None, Some(&env), marker), Check::Gone);
        assert_eq!(
            check(
                500,
                Some(&stat_line("sleep", 'Z', 10, 500)),
                Some(b""),
                marker
            ),
            Check::Zombie
        );
    }

    /// Kills its child on drop, so a failed assertion leaves nothing running.
    struct Reaped(Child);
    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn the_kill_signals_only_a_process_that_passes_the_recheck() {
        // A marker no other process carries: this test's PID and clock.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let runtime = format!("/nonexistent/sweep-test-{}-{nanos}", std::process::id());
        let marker = format!("XDG_RUNTIME_DIR={runtime}");
        let mut child = Reaped(
            Command::new("sleep")
                .arg("600")
                .env("XDG_RUNTIME_DIR", &runtime)
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id() as i32;
        // exec replaces the environment the scan reads; wait until it is sleep's.
        let found = (0..1000)
            .find_map(|_| {
                let hit = scan(&marker)
                    .unwrap()
                    .marked
                    .into_iter()
                    .find(|m| m.pid == pid && m.args.starts_with("sleep"));
                if hit.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                hit
            })
            .expect("scan did not find the marked child");
        assert_eq!(
            scan(&marker).unwrap().marked.len(),
            1,
            "only the child carries the marker"
        );

        // Recorded with another start time, as a reused PID would show.
        let stale = Marked {
            start_time: found.start_time + 1,
            ..found.clone()
        };
        assert_eq!(
            kill(std::slice::from_ref(&stale), &marker),
            [(
                pid,
                Outcome::Skipped(Check::StartTimeChanged {
                    now: found.start_time
                })
            )]
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "a skipped process must still run"
        );

        assert_eq!(kill(&[found], &marker), [(pid, Outcome::Killed)]);
        let status = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
