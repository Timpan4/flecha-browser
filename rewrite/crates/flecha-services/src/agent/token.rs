//! The agent bridge's bearer token.
//!
//! The Chromium shell kept it in the profile's saved state, which the profile directory's
//! permissions protect. Here it lives in its own file that only the owner can read, so state
//! exports, imports and set-aside copies never carry it. As in the shell, it is 24 random bytes
//! in unpadded base64url, made when none exists and replaced on request.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use subtle::ConstantTimeEq;

/// `newToken` in `chromium/shell/lib/settingsCore.ts`.
const TOKEN_BYTES: usize = 24;
/// Unpadded base64 of `TOKEN_BYTES`.
const TOKEN_CHARS: usize = TOKEN_BYTES.div_ceil(3) * 4;

pub struct AgentToken {
    path: PathBuf,
    /// `Bearer <token>`, the whole `Authorization` value a request must carry.
    header: RwLock<Vec<u8>>,
}

impl fmt::Debug for AgentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentToken")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AgentToken {
    /// Reads the token at `path`, or writes a new one there. A file that is not a regular file
    /// owned by this user, that others may read or write, or that holds no well-formed token is
    /// replaced: its token may have leaked.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let token = match read_private(&path)? {
            Some(token) => token,
            None => {
                let token = generate()?;
                write_private(&path, &token)?;
                token
            }
        };
        Ok(Self {
            header: RwLock::new(bearer(&token)),
            path,
        })
    }

    /// Replaces the token. Requests with the old one fail from the next request on.
    pub fn renew(&self) -> io::Result<()> {
        let token = generate()?;
        write_private(&self.path, &token)?;
        *self.header.write().unwrap_or_else(|err| err.into_inner()) = bearer(&token);
        Ok(())
    }

    /// The token, for Settings to show the user.
    pub fn reveal(&self) -> String {
        let header = self.header.read().unwrap_or_else(|err| err.into_inner());
        String::from_utf8_lossy(&header["Bearer ".len()..]).into_owned()
    }

    /// Whether `authorization` is exactly `Bearer <token>`, compared in constant time.
    pub(crate) fn authorizes(&self, authorization: &[u8]) -> bool {
        let header = self.header.read().unwrap_or_else(|err| err.into_inner());
        header.len() > "Bearer ".len() && bool::from(header.as_slice().ct_eq(authorization))
    }
}

fn bearer(token: &str) -> Vec<u8> {
    format!("Bearer {token}").into_bytes()
}

fn generate() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn well_formed(token: &str) -> bool {
    token.len() == TOKEN_CHARS
        && URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|bytes| bytes.len() == TOKEN_BYTES)
}

#[cfg(unix)]
fn read_private(path: &Path) -> io::Result<Option<String>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        // A symlink fails with ELOOP under O_NOFOLLOW; it is replaced like any untrusted file.
        Err(err)
            if err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ELOOP) =>
        {
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let meta = file.metadata()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let owner = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.uid() != owner || meta.mode() & 0o077 != 0 {
        return Ok(None);
    }
    let mut token = String::new();
    if file
        .take(TOKEN_CHARS as u64 + 1)
        .read_to_string(&mut token)
        .is_err()
    {
        return Ok(None);
    }
    Ok(well_formed(&token).then_some(token))
}

/// Writes a sibling file with mode 0600 and renames it over `path`, so the token is never
/// readable by others and a crash leaves the old token or the new one.
#[cfg(unix)]
fn write_private(path: &Path, token: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "token path has no file name"))?
        .to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    let temp = dir.join(name);
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

#[cfg(not(unix))]
fn read_private(_: &Path) -> io::Result<Option<String>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the agent token needs owner-only file permissions, implemented for Unix only",
    ))
}

#[cfg(not(unix))]
fn write_private(_: &Path, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the agent token needs owner-only file permissions, implemented for Unix only",
    ))
}
