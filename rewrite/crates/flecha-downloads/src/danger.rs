//! Which downloads are held for a keep-or-discard decision.
//!
//! Patch 0058 reports two kinds, insecure first: `WarningKind` asks `IsInsecure()` before
//! `IsDangerous()`. CEF's `DownloadItem` exposes neither, so the rules are ported here:
//!
//! - dangerous: `DownloadTargetDeterminer::GetDangerLevel` over the vendored file-type
//!   policy (`chrome/browser/download/download_target_determiner.cc`);
//! - insecure: `GetInsecureDownloadStatusForDownload`
//!   (`chrome/browser/download/insecure_download_blocking.cc`) with the feature parameters
//!   Chromium 158 compiles in. `fieldtrial_testing_config.json` has no entry for
//!   `TreatUnsafeDownloadsAsActive`.
//!
//! Safe Browsing verdicts, enterprise policies, auto-open types and trusted extension sources
//! are out of scope: Flecha has none of them.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::policy::{DangerLevel, FileTypePolicies};

/// Why a download waits for the person to keep or discard it (patch 0058's `kind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Warning {
    /// Its file type is dangerous.
    Dangerous,
    /// It came over plain HTTP, or an insecure page started it.
    Insecure,
}

/// What CEF's `DownloadItem` can't say about how a download started. Every field is required:
/// a guess here changes which files get a warning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DangerSignals {
    /// A user gesture started the request (`DownloadItem::HasUserGesture`).
    pub has_user_gesture: bool,
    /// The navigation came from the address bar (`PAGE_TRANSITION_FROM_ADDRESS_BAR`).
    pub from_address_bar: bool,
    /// History has a visit to the referrer's host from before today's local midnight
    /// (`VisitCountsToVisitedBefore`).
    pub visited_referrer_before: bool,
}

/// `GetDangerLevel`, for a download that is not prompted for a path. A prompted download is
/// approved by the person who picked the path, so it is never dangerous; see
/// [`crate::decide`].
pub fn file_danger(
    policies: &FileTypePolicies,
    file_name: &str,
    signals: DangerSignals,
) -> DangerLevel {
    let level = policies.danger_level(file_name);
    if level == DangerLevel::AllowOnUserGesture
        && (signals.from_address_bar
            || (signals.has_user_gesture && signals.visited_referrer_before))
    {
        return DangerLevel::NotDangerous;
    }
    level
}

/// How a download started, for the insecure check. `DownloadSource` and the page transition
/// in Chromium.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trigger {
    /// A page navigation or link the person followed. `typed` and `from_address_bar` matter
    /// only to mixed-content exemptions; `reload` and `from_api` exempt it entirely.
    Navigation {
        reload: bool,
        typed: bool,
        from_address_bar: bool,
        forward_back: bool,
        auto_toplevel: bool,
        auto_bookmark: bool,
        from_api: bool,
    },
    /// "Save link as" and friends (`DownloadSource::CONTEXT_MENU`).
    ContextMenu,
    /// `WebContents::Download` (`DownloadSource::WEB_CONTENTS_API`), e.g. "Save image as".
    WebContentsApi,
    /// A retry of an earlier download (`DownloadSource::RETRY`).
    Retry,
    /// Browser-internal downloads (`DownloadSource::INTERNAL_API`).
    Internal,
}

/// The URLs that decide whether a download was delivered securely.
#[derive(Clone, Copy, Debug)]
pub struct InsecureSignals<'a> {
    /// The URL the bytes came from: the end of the redirect chain (`GetURL`).
    pub download_url: &'a Url,
    /// The URLs before it in the redirect chain, oldest first; empty without redirects.
    pub redirects: &'a [Url],
    /// The frame that asked for the download, or `None` when unknown. Chromium falls back to
    /// the tab's URL when the request has no initiator; pass that here.
    pub initiator: Option<&'a Url>,
    pub trigger: Trigger,
    /// Chromium's HTTPS-First Mode pref (`kHttpsOnlyModeEnabled`).
    pub https_first_mode: bool,
}

/// `InsecureDownloadStatus` with Chromium 158's compiled-in parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsecureVerdict {
    Safe,
    /// Hold it for a decision (`BLOCK`, shown as a bypassable warning).
    Hold,
    /// Cancel it without asking (`SILENT_BLOCK`).
    SilentBlock,
}

/// `kSafeExtensions`: types that don't warn when delivered insecurely, unless HTTPS-First
/// Mode is on.
const SAFE_EXTENSIONS: &[&str] = &[
    "txt", "css", "json", "csv", "tsv", "jpg", "jpeg", "png", "gif", "tif", "tiff", "ico", "webp",
    "avif", "aac", "midi", "ogg", "wav", "webm", "mp3", "mp4", "m4a", "flac", "mpeg", "mov", "wmv",
];

/// `kSilentBlockExtensionList`, an allowlist by default (`TreatSilentBlockListAsAllowlist`).
const SILENTLY_UNBLOCKED: &[&str] = &["silently_unblocked_for_testing"];

/// `GetInsecureDownloadStatusForDownload`. `target_name` is the file name being saved; its
/// final extension picks the lists.
pub fn insecure_verdict(target_name: &str, signals: InsecureSignals<'_>) -> InsecureVerdict {
    let download_url = signals.download_url;
    // InsecureDownloadData takes `FinalExtension()` as is; ContainsExtension compares it
    // ignoring ASCII case.
    let extension = match target_name.rfind('.') {
        Some(dot) if target_name != "." && target_name != ".." => {
            target_name[dot + 1..].to_ascii_lowercase()
        }
        _ => String::new(),
    };
    let chain_secure = signals.redirects.iter().all(is_url_potentially_trustworthy);
    let delivered_securely = chain_secure
        && (is_url_potentially_trustworthy(download_url)
            || matches!(download_url.scheme(), "blob" | "file"));

    let exempt_from_insecure = match signals.trigger {
        Trigger::Retry | Trigger::Internal => true,
        Trigger::Navigation {
            reload, from_api, ..
        } => reload || from_api,
        Trigger::ContextMenu | Trigger::WebContentsApi => false,
    };
    // Chromium holds the initiator as a `url::Origin` and reads it through `GetURL()`.
    let initiator = signals.initiator.and_then(origin_url);
    let initiator_insecure = initiator
        .as_ref()
        .is_some_and(|origin| !is_url_potentially_trustworthy(origin));
    let is_insecure = !exempt_from_insecure
        && (initiator_insecure || !delivered_securely)
        && !is_localhost(download_url);
    if !is_insecure {
        return InsecureVerdict::Safe;
    }

    let exempt_from_mixed = match signals.trigger {
        Trigger::Retry | Trigger::Internal => true,
        Trigger::Navigation {
            reload,
            typed,
            from_address_bar,
            forward_back,
            auto_toplevel,
            auto_bookmark,
            from_api,
        } => {
            reload
                || typed
                || from_address_bar
                || forward_back
                || auto_toplevel
                || auto_bookmark
                || from_api
        }
        Trigger::ContextMenu | Trigger::WebContentsApi => false,
    };
    let is_mixed_content = !exempt_from_mixed
        && initiator
            .as_ref()
            .is_some_and(|origin| matches!(origin.scheme(), "https" | "wss"))
        && !delivered_securely;

    if !is_mixed_content {
        if !signals.https_first_mode && SAFE_EXTENSIONS.contains(&extension.as_str()) {
            return InsecureVerdict::Safe;
        }
        return InsecureVerdict::Hold;
    }
    // Mixed content with TreatUnsafeDownloadsAsActive on (its default). The silent-block list
    // is an allowlist, so everything else is silently blocked unless the person asked for it.
    if !SILENTLY_UNBLOCKED.contains(&extension.as_str()) {
        return match signals.trigger {
            Trigger::ContextMenu | Trigger::WebContentsApi => InsecureVerdict::Hold,
            _ => InsecureVerdict::SilentBlock,
        };
    }
    // The block list is empty and not an allowlist; the warn list likewise.
    InsecureVerdict::Safe
}

/// `network::IsUrlPotentiallyTrustworthy` (`net/base/is_potentially_trustworthy.cc`), without
/// embedder-registered secure schemes or the secure-origin allowlist switch.
pub fn is_url_potentially_trustworthy(url: &Url) -> bool {
    let scheme = url.scheme();
    if scheme == "about" && matches!(url.path(), "blank" | "srcdoc") {
        return true;
    }
    if scheme == "data" {
        return true;
    }
    let origin_url = if scheme == "blob" || scheme == "filesystem" {
        match Url::parse(url.path()) {
            Ok(inner) => inner,
            Err(_) => return false,
        }
    } else {
        url.clone()
    };
    match origin_url.scheme() {
        "https" | "wss" => true,
        "file" => true,
        "http" | "ws" => is_localhost(&origin_url),
        _ => false,
    }
}

/// `url::Origin::Create(url).GetURL()`: the URL of `url`'s origin, `None` when the origin is
/// opaque. `blob:` and `filesystem:` URLs take the origin of the URL inside them.
fn origin_url(url: &Url) -> Option<Url> {
    let inner;
    let url = if matches!(url.scheme(), "blob" | "filesystem") {
        inner = Url::parse(url.path()).ok()?;
        &inner
    } else {
        url
    };
    match url.scheme() {
        "file" => Url::parse("file:///").ok(),
        "http" | "https" | "ws" | "wss" | "ftp" => {
            Url::parse(&url.origin().ascii_serialization()).ok()
        }
        _ => None,
    }
}

/// `net::IsLocalhost`: a loopback IP literal, `localhost`, or a `.localhost` name.
fn is_localhost(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip == Ipv6Addr::LOCALHOST,
        Some(Host::Domain(name)) => {
            let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
            if let Ok(ip) = name.parse::<Ipv4Addr>() {
                return ip.is_loopback();
            }
            name == "localhost" || name.ends_with(".localhost")
        }
        None => false,
    }
}
