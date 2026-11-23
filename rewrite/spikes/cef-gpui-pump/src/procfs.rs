//! The `/proc` and `ss` text the isolation scan relies on. A scan that skips
//! what it cannot read, or truncates a path, could call a leaking run clean.

use std::path::Path;

/// The pathname of one `/proc/PID/maps` line, or `None` for an anonymous map.
/// The first five fields (address, perms, offset, dev, inode) never contain
/// spaces; everything after them is the pathname, spaces included.
pub fn maps_path(line: &str) -> Option<&str> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        rest = &rest[end..];
    }
    let path = rest.trim_start();
    (!path.is_empty()).then_some(path)
}

/// One `ss -xan` row: `Netid State Recv-Q Send-Q Local Inode Peer PeerInode`.
/// Returns the local address, its inode and the peer's inode. The local
/// address may contain spaces, so it is everything between the four leading
/// fields and the last three. iproute2's `ss` prints the peer address of a
/// Unix socket as the single token `*`.
pub fn ss_unix_row(line: &str) -> Option<(&str, u64, u64)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let n = fields.len();
    if n < 8 {
        return None;
    }
    let peer_inode = fields[n - 1].parse().ok()?;
    let inode = fields[n - 3].parse().ok()?;
    let offset = |s: &str| s.as_ptr() as usize - line.as_ptr() as usize;
    let start = offset(fields[4]);
    let end = offset(fields[n - 4]) + fields[n - 4].len();
    Some((&line[start..end], inode, peer_inode))
}

/// `ss -xan` output: the rows that parsed, and every other non-blank line.
#[derive(Debug, Default)]
pub struct SsTable {
    /// Local address, inode and peer inode of each row.
    pub rows: Vec<(String, u64, u64)>,
    pub rejected: Vec<String>,
}

/// The columns of `ss -xan`'s header, as iproute2 7.3 prints them.
const SS_HEADER: [&str; 8] = [
    "Netid",
    "State",
    "Recv-Q",
    "Send-Q",
    "Local",
    "Address:Port",
    "Peer",
    "Address:Port",
];

/// Parses `ss -xan` output. The first line must be the full header and every
/// other non-blank line a row: a rejected row is a socket whose address the
/// scan never checks, and a truncated header may mean truncated output, so the
/// caller must treat any rejected line as an incomplete scan.
pub fn parse_ss_unix(text: &str) -> SsTable {
    let mut table = SsTable::default();
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header.split_whitespace().eq(SS_HEADER) => {}
        Some(other) => table.rejected.push(other.to_string()),
        None => table
            .rejected
            .push("(no output, not even a header)".to_string()),
    }
    for line in lines.filter(|l| !l.trim().is_empty()) {
        match ss_unix_row(line) {
            Some((address, inode, peer)) => table.rows.push((address.to_string(), inode, peer)),
            None => table.rejected.push(line.to_string()),
        }
    }
    table
}

/// Parses the `:`-separated scan roots of `--pump-{option}`. The scan compares
/// the kernel's paths by whole components, so each entry must be absolute and
/// in normal form: `/run//user/1000` or `/run/user/./1000` would never match.
/// The kernel reports fd targets and mapped files with symlinks resolved, so an
/// entry that exists is also added in its canonical form, and a symlink alias
/// then matches what it points to. An entry that does not exist stays as given;
/// any other resolution error, or a canonical path that is not valid UTF-8 and
/// so could not be matched exactly, is an error.
pub fn scan_roots(option: &str, value: &str) -> Result<Vec<String>, String> {
    let mut roots = Vec::new();
    for p in value.split(':').filter(|p| !p.is_empty()) {
        if !p.starts_with('/') {
            return Err(format!(
                "--pump-{option} entry {p:?} is not an absolute path"
            ));
        }
        let normal = p == "/"
            || (!p.ends_with('/')
                && p[1..]
                    .split('/')
                    .all(|c| !c.is_empty() && c != "." && c != ".."));
        if !normal {
            return Err(format!(
                "--pump-{option} entry {p:?} is not in normal form \
                 (no empty, `.` or `..` components and no trailing `/`)"
            ));
        }
        roots.push(p.to_string());
        let real = match std::fs::canonicalize(p) {
            Ok(real) => real,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("--pump-{option} entry {p:?}: {e}")),
        };
        let Some(real) = real.to_str().map(str::to_string) else {
            return Err(format!(
                "--pump-{option} entry {p:?} resolves to {real:?}, which is not valid UTF-8"
            ));
        };
        if !roots.contains(&real) {
            roots.push(real);
        }
    }
    Ok(roots)
}

/// The targets of one `/proc/PID/fd` directory. Every entry that could not be
/// listed or resolved is a failure, whatever the reason: a descriptor closed
/// mid-scan and one the scan may not read look the same from here, and either
/// could have pointed at a forbidden path.
#[derive(Debug, Default)]
pub struct FdTable {
    pub targets: Vec<String>,
    pub failures: Vec<String>,
}

impl FdTable {
    pub fn complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Reads an fd directory. Fails only if the directory itself cannot be opened.
pub fn read_fd_table(dir: &Path) -> std::io::Result<FdTable> {
    let mut table = FdTable::default();
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                table.failures.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        match std::fs::read_link(entry.path()) {
            Ok(target) => table.targets.push(target.to_string_lossy().into_owned()),
            Err(e) => table
                .failures
                .push(format!("{}: {e}", entry.path().display())),
        }
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_under;

    #[test]
    fn maps_paths_keep_their_spaces() {
        let line = "7f00a000-7f00b000 r--p 00000000 103:02 1234                      /home/Test User/.cache/x";
        let path = maps_path(line).unwrap();
        assert_eq!(path, "/home/Test User/.cache/x");
        assert!(path_under(path, "/home/Test User"));
        assert_eq!(
            maps_path("7f00a000-7f00b000 rw-p 00000000 00:00 0    [heap]"),
            Some("[heap]")
        );
        assert_eq!(maps_path("7f00a000-7f00b000 rw-p 00000000 00:00 0"), None);
        assert_eq!(
            maps_path("7f00a000-7f00b000 rw-p 00000000 00:00 0   "),
            None
        );
    }

    #[test]
    fn ss_rows_keep_spaces_in_the_local_address() {
        let row = "u_str ESTAB 0      0      /home/Test User/run/bus 4242 * 4343";
        let (local, inode, peer) = ss_unix_row(row).unwrap();
        assert_eq!(local, "/home/Test User/run/bus");
        assert!(path_under(local, "/home/Test User"));
        assert_eq!((inode, peer), (4242, 4343));
        assert_eq!(
            ss_unix_row("u_str ESTAB 0 0 * 17 * 18"),
            Some(("*", 17, 18))
        );
        assert_eq!(
            ss_unix_row("Netid State Recv-Q Send-Q Local Address:Port Peer Address:Port"),
            None
        );
    }

    /// Review thread 4666314421: a row the parser rejects is reported, so the
    /// scan can fail instead of leaving that socket unchecked.
    #[test]
    fn an_unparseable_ss_row_is_rejected_not_skipped() {
        let text = "Netid State Recv-Q Send-Q Local Address:Port Peer Address:Port\n\
                    u_str ESTAB 0 0 /run/user/1000/bus 17 * 18\n\
                    \n\
                    u_str ESTAB 0 0 /run/user/1000/bus 19 *\n";
        let table = parse_ss_unix(text);
        assert_eq!(table.rows, [("/run/user/1000/bus".to_string(), 17, 18)]);
        assert_eq!(table.rejected, ["u_str ESTAB 0 0 /run/user/1000/bus 19 *"]);
        let headless = parse_ss_unix("u_str ESTAB 0 0 * 17 * 18\n");
        assert_eq!(headless.rejected.len(), 1, "a missing header is rejected");
        // Review thread 4666506440: a truncated header is not an empty table.
        let truncated = parse_ss_unix("Netid State\n");
        assert_eq!(truncated.rejected, ["Netid State"]);
        let spaced = parse_ss_unix(
            "Netid State  Recv-Q Send-Q      Local Address:Port    Peer Address:Port\n",
        );
        assert!(spaced.rejected.is_empty(), "{spaced:?}");
        assert_eq!(
            parse_ss_unix("").rejected.len(),
            1,
            "empty output is rejected"
        );
    }

    /// Review thread 4666314387: spellings the scan could never match are
    /// rejected, naming the option.
    #[test]
    fn scan_roots_must_be_absolute_and_normal() {
        for bad in [
            "run/user/1000",
            "/run//user/1000",
            "/run/user/./1000",
            "/run/user/../user/1000",
            "/run/user/1000/",
        ] {
            let error = scan_roots("forbid-paths", bad).unwrap_err();
            assert!(error.contains("--pump-forbid-paths"), "{bad}: {error}");
        }
        assert_eq!(
            scan_roots("allow-paths", "/nonexistent-cef-gpui-pump/a::/").unwrap(),
            ["/nonexistent-cef-gpui-pump/a", "/"]
        );
    }

    /// Review thread 4666314387: a symlink alias also covers its target, the
    /// path the kernel reports.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_scan_root_also_covers_its_target() {
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "cef-gpui-pump-scan-roots-test-{}",
                std::process::id()
            ));
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        let alias = dir.join("alias").to_string_lossy().into_owned();
        let real = dir.join("real").to_string_lossy().into_owned();
        let roots = scan_roots("forbid-paths", &alias);
        std::fs::remove_dir_all(&dir).unwrap();
        let roots = roots.unwrap();
        assert_eq!(roots, [alias, real.clone()]);
        assert!(roots.iter().any(|r| path_under(&format!("{real}/bus"), r)));
    }

    /// Review threads 4666506455 and 4666506429: a root that cannot be resolved
    /// for a reason other than not existing, or that resolves to a path that is
    /// not valid UTF-8, is an error naming the option.
    #[cfg(unix)]
    #[test]
    fn unresolvable_or_non_utf8_scan_roots_are_errors() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "cef-gpui-pump-root-errors-test-{}",
                std::process::id()
            ));
        std::fs::create_dir(&dir).unwrap();
        let bytes = std::ffi::OsStr::from_bytes(b"not-utf8-\xff");
        std::fs::create_dir(dir.join(bytes)).unwrap();
        std::os::unix::fs::symlink(dir.join(bytes), dir.join("alias")).unwrap();
        std::fs::create_dir(dir.join("locked")).unwrap();
        std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        let alias = dir.join("alias").to_string_lossy().into_owned();
        let hidden = dir.join("locked/x").to_string_lossy().into_owned();
        let non_utf8 = scan_roots("allow-paths", &alias);
        let denied = scan_roots("forbid-paths", &hidden);
        // A process that may read a mode 000 dir (root) cannot test the error.
        let privileged = std::fs::read_dir(dir.join("locked")).is_ok();
        std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let error = non_utf8.unwrap_err();
        assert!(
            error.contains("--pump-allow-paths") && error.contains("UTF-8"),
            "{error}"
        );
        if !privileged {
            let error = denied.unwrap_err();
            assert!(error.contains("--pump-forbid-paths"), "{error}");
        }
    }

    /// Review thread probe.rs:284: an entry that cannot be resolved makes the
    /// table incomplete instead of being skipped.
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_fd_entry_makes_the_table_incomplete() {
        let dir = std::env::temp_dir().join(format!(
            "cef-gpui-pump-fd-table-test-{}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink("/run/user/1000/bus", dir.join("3")).unwrap();
        // readlink on a regular file fails (EINVAL), as it would for an entry
        // that vanished or cannot be read.
        std::fs::write(dir.join("4"), "").unwrap();
        let table = read_fd_table(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let table = table.unwrap();
        assert_eq!(table.targets, ["/run/user/1000/bus"]);
        assert_eq!(table.failures.len(), 1, "{table:?}");
        assert!(!table.complete());
        assert!(
            read_fd_table(&dir).is_err(),
            "a missing directory is an error"
        );
    }
}
