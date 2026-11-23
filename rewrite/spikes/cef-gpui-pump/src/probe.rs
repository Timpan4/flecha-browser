//! Linux `/proc` reads for the done checks: the process tree, its CPU time, its
//! command lines and the sockets it listens on.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    /// Field 22 of `/proc/PID/stat`, in clock ticks since boot. With the PID it
    /// identifies a process even after the PID is reused.
    pub start_time: u64,
    pub utime: u64,
    pub stime: u64,
    pub cmdline: Vec<String>,
    /// Sum of `/proc/PID/task/*/schedstat` run time, in nanoseconds. Finer
    /// than utime + stime, which count in clock ticks.
    pub run_ns: u64,
    /// The process's PID inside its innermost PID namespace (last `NSpid` value).
    pub inner_pid: Option<i32>,
    /// Parent's and grandparent's `--type=` values and the parent's inner PID, filled in by `tree`.
    pub parent_type: Option<String>,
    pub grandparent_type: Option<String>,
    pub parent_inner_pid: Option<i32>,
}

impl Proc {
    fn switch_type(&self) -> Option<String> {
        self.cmdline
            .iter()
            .find_map(|arg| arg.strip_prefix("--type=").map(str::to_string))
    }

    /// Chromium's `--type=` switch, refined for zygote-forked processes.
    ///
    /// Processes forked from a zygote keep `--type=zygote` on this build. The
    /// observed layout (scripts/run-checks.sh, devtools tree_while_open) is:
    /// browser -> sandboxed zygote (PID-namespace init, inner PID 1) -> zygote
    /// worker -> one process per renderer, each in its own new PID namespace;
    /// and browser -> unsandboxed zygote -> its forked children.
    pub fn kind(&self) -> String {
        if self.cmdline.is_empty() {
            return "exiting".to_string();
        }
        let zygote = |t: &Option<String>| t.as_deref() == Some("zygote");
        match self.switch_type() {
            Some(t) if t == "zygote" && zygote(&self.parent_type) => {
                if zygote(&self.grandparent_type) {
                    "zygote-forked".to_string()
                } else if self.parent_inner_pid == Some(1) {
                    "zygote".to_string()
                } else {
                    "zygote-forked-unsandboxed".to_string()
                }
            }
            Some(t) => t,
            None if self.parent_type.is_none() && self.grandparent_type.is_none() => {
                "browser".to_string()
            }
            None => format!(
                "other:{}",
                self.cmdline[0].rsplit('/').next().unwrap_or_default()
            ),
        }
    }

    pub fn summary(&self) -> Value {
        json!({
            "pid": self.pid,
            "ppid": self.ppid,
            "type": self.kind(),
            "start_time": self.start_time,
            "inner_pid": self.inner_pid,
        })
    }
}

fn run_ns(pid: i32) -> u64 {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return 0;
    };
    tasks
        .flatten()
        .filter_map(|t| fs::read_to_string(t.path().join("schedstat")).ok())
        .filter_map(|s| s.split_whitespace().next()?.parse::<u64>().ok())
        .sum()
}

fn inner_pid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("NSpid:"))?;
    line.split_whitespace().last()?.parse().ok()
}

fn read_proc(pid: i32) -> Option<Proc> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm (field 2) may contain spaces and parentheses; fields resume after the last ')'.
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split(' ').collect();
    // `rest` starts at field 3 (state), so field N is fields[N - 3].
    let ppid = fields.get(1)?.parse().ok()?;
    let utime = fields.get(11)?.parse().ok()?;
    let stime = fields.get(12)?.parse().ok()?;
    let start_time = fields.get(19)?.parse().ok()?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default();
    Some(Proc {
        pid,
        ppid,
        start_time,
        utime,
        stime,
        cmdline,
        run_ns: run_ns(pid),
        inner_pid: inner_pid(pid),
        parent_type: None,
        grandparent_type: None,
        parent_inner_pid: None,
    })
}

fn all_procs() -> Vec<Proc> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(read_proc)
        .collect()
}

/// `root` and every process descended from it.
pub fn tree(root: i32) -> Vec<Proc> {
    let procs = all_procs();
    let mut children: HashMap<i32, Vec<&Proc>> = HashMap::new();
    for p in &procs {
        children.entry(p.ppid).or_default().push(p);
    }
    let mut out: Vec<Proc> = procs.iter().filter(|p| p.pid == root).cloned().collect();
    let mut i = 0;
    while i < out.len() {
        let parent = &out[i];
        let (parent_type, grandparent_type, parent_inner) = (
            parent.switch_type(),
            parent.parent_type.clone(),
            parent.inner_pid,
        );
        // The root has no recorded parent; mark its children so `kind` can tell
        // the root ("browser") from other processes with no --type.
        let parent_type = parent_type.or_else(|| Some("browser".to_string()));
        if let Some(kids) = children.get(&out[i].pid) {
            let kids: Vec<Proc> = kids
                .iter()
                .map(|p| Proc {
                    parent_type: parent_type.clone(),
                    grandparent_type: grandparent_type.clone(),
                    parent_inner_pid: parent_inner,
                    ..(*p).clone()
                })
                .collect();
            out.extend(kids);
        }
        i += 1;
    }
    out
}

pub fn self_pid() -> i32 {
    std::process::id() as i32
}

pub fn clock_ticks_per_second() -> u64 {
    // SAFETY: sysconf has no preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    ticks as u64
}

/// True when a process with this PID and start time still exists and is not a zombie.
pub fn alive(pid: i32, start_time: u64) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(close) = stat.rfind(')') else {
        return false;
    };
    let fields: Vec<&str> = stat[close + 2..].split(' ').collect();
    fields.first() != Some(&"Z")
        && fields.get(19).and_then(|s| s.parse::<u64>().ok()) == Some(start_time)
}

/// CPU used by each process between two tree snapshots, matched on PID and start time.
/// Processes that started or exited inside the window count only the ticks seen.
pub fn cpu_delta(before: &[Proc], after: &[Proc], wall_secs: f64) -> Value {
    let hz = clock_ticks_per_second() as f64;
    let start: HashMap<(i32, u64), &Proc> =
        before.iter().map(|p| ((p.pid, p.start_time), p)).collect();
    let mut rows = Vec::new();
    let mut total_ticks = 0u64;
    let mut total_ns = 0u64;
    for p in after {
        let (u0, s0, ns0) = start
            .get(&(p.pid, p.start_time))
            .map(|b| (b.utime, b.stime, b.run_ns))
            .unwrap_or((0, 0, 0));
        let ticks = (p.utime + p.stime).saturating_sub(u0 + s0);
        // Threads that exited inside the window drop out of task/*/schedstat,
        // so this can undercount; saturate rather than go negative.
        let ns = p.run_ns.saturating_sub(ns0);
        total_ticks += ticks;
        total_ns += ns;
        rows.push(json!({
            "pid": p.pid,
            "type": p.kind(),
            "cpu_ticks": ticks,
            "run_ms": ns as f64 / 1e6,
            "cpu_percent_of_one_core": ns as f64 / 1e9 / wall_secs * 100.0,
        }));
    }
    let exited: Vec<Value> = before
        .iter()
        .filter(|b| {
            !after
                .iter()
                .any(|a| a.pid == b.pid && a.start_time == b.start_time)
        })
        .map(Proc::summary)
        .collect();
    json!({
        "wall_seconds": wall_secs,
        "clock_ticks_per_second": hz,
        "total_cpu_ticks": total_ticks,
        "total_cpu_percent_of_one_core_from_ticks": total_ticks as f64 / hz / wall_secs * 100.0,
        "total_run_ms": total_ns as f64 / 1e6,
        "total_cpu_percent_of_one_core": total_ns as f64 / 1e9 / wall_secs * 100.0,
        "method": "sum over the process tree of /proc/PID/task/*/schedstat run time (ns); ticks from /proc/PID/stat utime+stime",
        "processes": rows,
        "exited_during_window": exited,
    })
}

/// Arguments in the tree's command lines that would open a DevTools endpoint.
pub fn devtools_switches(procs: &[Proc]) -> Vec<Value> {
    let mut hits = Vec::new();
    for p in procs {
        for arg in &p.cmdline {
            if arg.contains("remote-debugging") || arg.contains("enable-automation") {
                hits.push(json!({ "pid": p.pid, "type": p.kind(), "arg": arg }));
            }
        }
    }
    hits
}

/// Listening TCP and Unix sockets owned by the given processes.
///
/// Sandboxed children are not dumpable, so their fd tables can be unreadable.
/// Those PIDs are returned in `unreadable` rather than silently skipped.
pub fn listening_sockets(procs: &[Proc]) -> Value {
    let mut owner: HashMap<u64, i32> = HashMap::new();
    let mut unreadable = Vec::new();
    for p in procs {
        let Ok(fds) = fs::read_dir(format!("/proc/{}/fd", p.pid)) else {
            unreadable.push(p.summary());
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = fs::read_link(fd.path()) {
                let target = target.to_string_lossy();
                if let Some(inode) = target
                    .strip_prefix("socket:[")
                    .and_then(|s| s.strip_suffix(']'))
                    && let Ok(inode) = inode.parse()
                {
                    owner.insert(inode, p.pid);
                }
            }
        }
    }
    let owned: HashSet<u64> = owner.keys().copied().collect();
    let mut listening = Vec::new();
    // /proc/net/tcp{,6}: field 3 is the state, 0A is TCP_LISTEN; field 9 is the inode.
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for line in fs::read_to_string(table)
            .unwrap_or_default()
            .lines()
            .skip(1)
        {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.get(3) == Some(&"0A")
                && let Some(inode) = f.get(9).and_then(|s| s.parse::<u64>().ok())
                && owned.contains(&inode)
            {
                listening.push(json!({ "table": table, "local": f[1], "pid": owner[&inode] }));
            }
        }
    }
    // /proc/net/unix: field 3 is flags; __SO_ACCEPTCON (0x10000) marks a listening socket.
    for line in fs::read_to_string("/proc/net/unix")
        .unwrap_or_default()
        .lines()
        .skip(1)
    {
        let f: Vec<&str> = line.split_whitespace().collect();
        let flags = f
            .get(3)
            .and_then(|s| u64::from_str_radix(s, 16).ok())
            .unwrap_or(0);
        let inode = f.get(6).and_then(|s| s.parse::<u64>().ok());
        if flags & 0x10000 != 0
            && let Some(inode) = inode.filter(|i| owned.contains(i))
        {
            listening
                .push(json!({ "table": "/proc/net/unix", "path": f.get(7), "pid": owner[&inode] }));
        }
    }
    json!({ "listening": listening, "sockets_seen": owned.len(), "unreadable_fd_tables": unreadable })
}

/// Waits until each `(pid, start_time)` has exited, bounded by `timeout`.
/// Uses `pidfd_open` + `poll`, which wake on exit, so there is no polling interval.
pub fn wait_gone(procs: &[(i32, u64)], timeout: std::time::Duration) -> Value {
    use std::time::Instant;
    let start = Instant::now();
    let mut rows = Vec::new();
    let mut waiting: Vec<(i32, u64, libc::c_int)> = Vec::new();
    for &(pid, start_time) in procs {
        // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
        if fd < 0 {
            rows.push(json!({ "pid": pid, "gone_after_ms": 0.0, "note": "gone before wait" }));
        } else if !alive(pid, start_time) {
            // The PID exists but is a zombie or a different process now.
            // SAFETY: fd came from pidfd_open above.
            unsafe { libc::close(fd) };
            rows.push(
                json!({ "pid": pid, "gone_after_ms": 0.0, "note": "gone or reused before wait" }),
            );
        } else {
            waiting.push((pid, start_time, fd));
        }
    }
    while !waiting.is_empty() {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        let mut fds: Vec<libc::pollfd> = waiting
            .iter()
            .map(|&(_, _, fd)| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let ms = remaining.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        // SAFETY: fds is a valid array of pollfd for its length.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
        if n < 0 {
            break;
        }
        let now_ms = start.elapsed().as_secs_f64() * 1e3;
        let mut still = Vec::new();
        for (entry, pfd) in waiting.into_iter().zip(&fds) {
            if pfd.revents != 0 {
                // SAFETY: fd came from pidfd_open above.
                unsafe { libc::close(entry.2) };
                rows.push(json!({ "pid": entry.0, "gone_after_ms": now_ms }));
            } else {
                still.push(entry);
            }
        }
        waiting = still;
    }
    let leaked: Vec<Value> = waiting
        .iter()
        .map(|&(pid, start_time, fd)| {
            // SAFETY: fd came from pidfd_open above.
            unsafe { libc::close(fd) };
            json!({ "pid": pid, "start_time": start_time })
        })
        .collect();
    json!({
        "timeout_ms": timeout.as_secs_f64() * 1e3,
        "exited": rows,
        "still_alive_at_timeout": leaked,
    })
}
