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

    /// Review thread probe.rs:284: an entry that cannot be resolved makes the
    /// table incomplete instead of being skipped.
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
