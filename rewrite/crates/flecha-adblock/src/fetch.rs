//! Downloading and caching the lists, ported from `fetchList` in
//! `chromium/shell/build-adblock.mjs`.
//!
//! A cached copy younger than [`MAX_AGE`] is used as is. An older one, or none, is downloaded
//! again; when that fails, a cached copy that still passes the checks is used, whatever its age.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};

use url::Url;

use crate::lists::{FilterList, LISTS, MAX_AGE, checked_text, js_trim};

/// One deadline for a download and all its redirects: `AbortSignal.timeout(30000)`.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// <https://fetch.spec.whatwg.org/#http-redirect-fetch>, as in build-adblock.mjs.
pub const MAX_REDIRECTS: u32 = 20;
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];
/// `std::io`'s default buffer size. It sets how often the length check runs, not a limit.
const READ_CHUNK: usize = 8 * 1024;

/// What a download sends: build-adblock.mjs asks for gzip and shares one deadline across
/// redirects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadRequest {
    pub accept_encoding: &'static str,
    pub deadline: Instant,
}

/// A response as it came off the wire: the body is not decompressed.
pub struct WireResponse {
    pub status: u16,
    /// Header names in any case, in the order received.
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl WireResponse {
    /// The first value, as Node keeps for Content-Length and Location.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every value joined with ", ", as Node reports other repeated headers.
    fn joined(&self, name: &str) -> Option<String> {
        let values: Vec<&str> = self
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }
}

/// The transport. Tests and the measurement use fakes; the app uses [`crate::UreqDownload`].
pub trait Download: Send + Sync {
    fn get(&self, url: &str, request: &DownloadRequest) -> io::Result<WireResponse>;
}

/// Where a list's text came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListSource {
    /// A cached copy used without fetching.
    Cache,
    Downloaded,
    /// The download failed; the cached copy, of any age, was used.
    CacheAfterFailedRefresh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedList {
    pub id: &'static str,
    pub text: String,
    pub source: ListSource,
}

/// Something that did not stop the list from loading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListWarning {
    InvalidCache { id: &'static str, error: String },
    UsedCache { id: &'static str, error: String },
}

impl std::fmt::Display for ListWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCache { id, error } => write!(f, "adblock: invalid cached {id}: {error}"),
            Self::UsedCache { id, error } => write!(
                f,
                "adblock: couldn't refresh {id} ({error}); using the cached copy"
            ),
        }
    }
}

/// A list that could not be loaded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListFailure {
    NoCache {
        id: &'static str,
        download: String,
    },
    InvalidCache {
        id: &'static str,
        download: String,
        cache: String,
    },
    CacheWrite {
        id: &'static str,
        error: String,
    },
    CacheRead {
        id: &'static str,
        error: String,
    },
}

impl std::fmt::Display for ListFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCache { id, download } => write!(
                f,
                "adblock: could not download {id} and no cache exists ({download})"
            ),
            Self::InvalidCache {
                id,
                download,
                cache,
            } => write!(
                f,
                "adblock: could not refresh {id} ({download}); invalid cache ({cache})"
            ),
            Self::CacheWrite { id, error } => {
                write!(f, "adblock: could not cache {id}: {error}")
            }
            Self::CacheRead { id, error } => {
                write!(f, "adblock: could not read cached {id}: {error}")
            }
        }
    }
}

impl std::error::Error for ListFailure {}

/// Both lists, fetched in parallel like build-adblock.mjs's `Promise.all`.
#[derive(Debug)]
pub struct FetchReport {
    /// Every list in [`LISTS`] order, or every list that failed.
    pub lists: Result<Vec<FetchedList>, Vec<ListFailure>>,
    pub warnings: Vec<ListWarning>,
}

pub fn fetch_lists(cache: &Path, refresh: bool, download: &dyn Download) -> FetchReport {
    FetchReport::from_attempts(fetch_each(|_, list| {
        attempt(list, refresh, cache, download)
    }))
}

pub(crate) fn fetch_each(
    fetch: impl Fn(usize, FilterList) -> ListAttempt + Sync,
) -> [ListAttempt; LISTS.len()] {
    std::thread::scope(|scope| {
        let handles = std::array::from_fn(|index| {
            let fetch = &fetch;
            scope.spawn(move || fetch(index, LISTS[index]))
        });
        handles.map(|handle| {
            handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    })
}

impl FetchReport {
    pub(crate) fn from_attempts(attempts: [ListAttempt; LISTS.len()]) -> Self {
        let mut warnings = Vec::new();
        let mut lists = Vec::new();
        let mut failures = Vec::new();
        for attempt in attempts {
            warnings.extend(attempt.warnings);
            match attempt.list {
                Ok(list) => lists.push(list),
                Err(failure) => failures.push(failure),
            }
        }
        Self {
            lists: if failures.is_empty() {
                Ok(lists)
            } else {
                Err(failures)
            },
            warnings,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Retry {
    Transport,
    Refresh,
}

pub(crate) struct ListAttempt {
    list: Result<FetchedList, ListFailure>,
    warnings: Vec<ListWarning>,
    pub retry: Option<Retry>,
}

pub fn cache_file(cache: &Path, id: &str) -> PathBuf {
    cache.join(format!("{id}.txt"))
}

/// The cached copy's age, zero when its time is in the future (`Date.now() - mtimeMs`).
pub fn cache_age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or_default())
}

pub fn fetch_list(
    list: FilterList,
    refresh: bool,
    cache: &Path,
    download: &dyn Download,
) -> (Result<FetchedList, ListFailure>, Vec<ListWarning>) {
    let attempt = attempt(list, refresh, cache, download);
    (attempt.list, attempt.warnings)
}

fn cached_text(list: FilterList, file: &Path) -> Result<String, String> {
    let bytes = fs::read(file).map_err(|error| error.to_string())?;
    checked_text(list.id, &bytes, false).map_err(|error| error.to_string())
}

pub(crate) fn cache_only(list: FilterList, cache: &Path) -> ListAttempt {
    let result = cached_text(list, &cache_file(cache, list.id))
        .map(|text| FetchedList {
            id: list.id,
            text,
            source: ListSource::Cache,
        })
        .map_err(|error| ListFailure::CacheRead { id: list.id, error });
    ListAttempt {
        list: result,
        warnings: Vec::new(),
        retry: None,
    }
}

pub(crate) fn attempt(
    list: FilterList,
    refresh: bool,
    cache: &Path,
    download: &dyn Download,
) -> ListAttempt {
    let mut warnings = Vec::new();
    let file = cache_file(cache, list.id);
    let age = cache_age(&file, SystemTime::now());
    let cached = || cached_text(list, &file);
    let fetched = |text, source| FetchedList {
        id: list.id,
        text,
        source,
    };
    if !refresh && age.is_some_and(|age| age < MAX_AGE) {
        match cached() {
            Ok(text) => {
                return ListAttempt {
                    list: Ok(fetched(text, ListSource::Cache)),
                    warnings,
                    retry: None,
                };
            }
            Err(error) => warnings.push(ListWarning::InvalidCache { id: list.id, error }),
        }
    }
    let text = match download_text(list, download) {
        Ok(text) => text,
        Err(download) => {
            let DownloadError { reason, retry } = download;
            let result = if age.is_some() {
                match cached() {
                    Ok(text) => {
                        warnings.push(ListWarning::UsedCache {
                            id: list.id,
                            error: reason,
                        });
                        Ok(fetched(text, ListSource::CacheAfterFailedRefresh))
                    }
                    Err(cache) => Err(ListFailure::InvalidCache {
                        id: list.id,
                        download: reason,
                        cache,
                    }),
                }
            } else {
                Err(ListFailure::NoCache {
                    id: list.id,
                    download: reason,
                })
            };
            return ListAttempt {
                list: result,
                warnings,
                retry: Some(retry),
            };
        }
    };
    let written = fs::create_dir_all(cache).and_then(|()| write_atomically(&file, text.as_bytes()));
    let result = match written {
        Ok(()) => Ok(fetched(text, ListSource::Downloaded)),
        Err(error) => Err(ListFailure::CacheWrite {
            id: list.id,
            error: error.to_string(),
        }),
    };
    let retry = result.is_err().then_some(Retry::Refresh);
    ListAttempt {
        list: result,
        warnings,
        retry,
    }
}

struct DownloadError {
    reason: String,
    retry: Retry,
}

impl DownloadError {
    fn transport(reason: impl ToString) -> Self {
        Self {
            reason: reason.to_string(),
            retry: Retry::Transport,
        }
    }

    fn rejected(reason: impl ToString) -> Self {
        Self {
            reason: reason.to_string(),
            retry: Retry::Refresh,
        }
    }
}

fn download_text(list: FilterList, download: &dyn Download) -> Result<String, DownloadError> {
    let request = DownloadRequest {
        accept_encoding: "gzip",
        deadline: Instant::now() + DOWNLOAD_TIMEOUT,
    };
    let mut response = follow_redirects(list.url, download, &request)?;
    if response.status != 200 {
        return Err(DownloadError::rejected(format!("HTTP {}", response.status)));
    }
    // Read the wire bytes. Decoding requires complete compression framing.
    let encoding = response.joined("content-encoding").map_or_else(
        || "identity".to_owned(),
        |value| js_trim(&value).to_ascii_lowercase(),
    );
    let compressed = match encoding.as_str() {
        "identity" => false,
        "gzip" | "deflate" | "br" => true,
        other => {
            return Err(DownloadError::rejected(format!(
                "unsupported Content-Encoding: {other}"
            )));
        }
    };
    let length = response.header("content-length").map(str::to_owned);
    let bytes = read_wire_bytes(&mut response.body, length.as_deref(), request.deadline)?;
    let bytes = if compressed {
        decode(&encoding, &bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                DownloadError::rejected("unexpected end of file")
            } else {
                DownloadError::rejected(error)
            }
        })?
    } else {
        bytes
    };
    checked_text(list.id, &bytes, !compressed && length.is_none()).map_err(DownloadError::rejected)
}

fn follow_redirects(
    url: &str,
    download: &dyn Download,
    request: &DownloadRequest,
) -> Result<WireResponse, DownloadError> {
    let mut url = url.to_owned();
    let mut redirects = 0;
    loop {
        if Instant::now() >= request.deadline {
            return Err(DownloadError::transport(
                "The operation was aborted due to timeout",
            ));
        }
        let response = download
            .get(&url, request)
            .map_err(DownloadError::transport)?;
        let location = match response.header("location") {
            Some(location) if REDIRECT_STATUSES.contains(&response.status) => location.to_owned(),
            _ => return Ok(response),
        };
        drop(response);
        if redirects == MAX_REDIRECTS {
            return Err(DownloadError::rejected("too many HTTP redirects"));
        }
        let target = Url::parse(&url)
            .and_then(|base| base.join(&location))
            .map_err(|_| DownloadError::rejected("Invalid URL"))?;
        if target.scheme() != "https" {
            return Err(DownloadError::rejected(
                "filter list redirect requires HTTPS",
            ));
        }
        url = target.into();
        redirects += 1;
    }
}

/// The body as received, checked against Content-Length: a longer body is refused before more
/// is read, and a shorter one at its end.
fn read_wire_bytes(
    body: &mut dyn Read,
    length: Option<&str>,
    deadline: Instant,
) -> Result<Vec<u8>, DownloadError> {
    const UNVERIFIED: &str = "could not verify Content-Length";
    // JavaScript's /^\d+$/ is ASCII; a length too large for u128 is never reached.
    let expected = match length {
        None => None,
        Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            Some(digits.parse::<u128>().unwrap_or(u128::MAX))
        }
        Some(_) => return Err(DownloadError::rejected(UNVERIFIED)),
    };
    let mut bytes = Vec::new();
    let mut chunk = vec![0; READ_CHUNK];
    loop {
        if Instant::now() >= deadline {
            return Err(DownloadError::transport(
                "The operation was aborted due to timeout",
            ));
        }
        let read = match body.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(DownloadError::transport(error)),
        };
        bytes.extend_from_slice(&chunk[..read]);
        if expected.is_some_and(|expected| bytes.len() as u128 > expected) {
            return Err(DownloadError::rejected("response exceeded Content-Length"));
        }
    }
    if expected.is_some_and(|expected| bytes.len() as u128 != expected) {
        return Err(DownloadError::transport(UNVERIFIED));
    }
    Ok(bytes)
}

/// Node's strict zlib decoders: a truncated stream is an error.
fn decode(encoding: &str, bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match encoding {
        "gzip" => {
            flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut out)?;
        }
        "deflate" => {
            flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out)?;
        }
        _ => {
            brotli_decompressor::BrotliDecompress(&mut &*bytes, &mut out)?;
        }
    }
    Ok(out)
}

/// Writes through a new file beside `path` and renames it into place, so a reader sees the old
/// or the new contents, never part of either.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let unique = format!(
        "{name}.{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temp = path.with_file_name(unique);
    let result = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
