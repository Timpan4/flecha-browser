//! Which pages a Boost applies to, from the shell's `lib/boostFiles.ts`.

use url::{Host, Url};

/// `boostMatches`: whether a Boost for `host` applies to the page at `url`.
///
/// `all` takes any URL that starts with `http:` or `https:`, compared as written. Any other host
/// is compared with the URL's WHATWG hostname: equal, or a subdomain of it. The stored host is
/// compared as stored, so an uppercase or Unicode host never matches. Ports and user info play
/// no part. This takes any scheme. Use [`applies`] for what the shell actually styled.
pub fn boost_matches(url: &str, host: &str) -> bool {
    if host == "all" {
        return starts_with_http(url);
    }
    hostname(url).is_some_and(|hostname| {
        hostname == host
            || hostname
                .strip_suffix(host)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// Whether the shell styled, reloaded or ran (through its execute path) a Boost for `host` on the
/// page at `url`.
///
/// The shell's manifest asks for `<all_urls>`, and `boostMatches` takes any scheme with a host,
/// such as `chrome://settings` for a Boost named `settings.css`. Chrome's scripting and user
/// script APIs refused every page that isn't http or https: internal, extension and `file:` pages
/// (the shell has no file access). CEF has no such layer, so the rewrite checks the scheme here.
pub fn applies(url: &str, host: &str) -> bool {
    is_web_page(url) && boost_matches(url, host)
}

/// Whether the user script the shell registered for a script Boost on `host` ran on `url`.
///
/// The shell registered `*://<host>/*` and `*://*.<host>/*` (`http://*/*` and `https://*/*` for
/// `all`). Chromium's `URLPattern` canonicalizes the pattern host (lowercase, punycode), trims
/// trailing dots from both hosts, and doesn't match subdomains of an IP address
/// (`extensions/common/url_pattern.cc`, `Parse` and `MatchesHost`). A script Boost ran where this
/// or [`applies`] matched.
pub fn user_script_matches(url: &str, host: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if host == "all" {
        return true;
    }
    let Some(pattern) = pattern_host(host) else {
        return false;
    };
    let pattern = pattern.trim_end_matches('.');
    let test = url.host_str().unwrap_or_default().trim_end_matches('.');
    if test == pattern {
        return true;
    }
    if matches!(url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_))) {
        return false;
    }
    test.len() > pattern.len() + 1
        && test.ends_with(pattern)
        && test.as_bytes()[test.len() - pattern.len() - 1] == b'.'
}

/// The canonical host of a registered pattern, or `None` when Chromium would reject it.
///
/// The shell saved only hosts matching `/^[a-z0-9.-]+$/i`, and only those are handled here.
/// Any other host is treated as a pattern Chromium rejects.
pub(crate) fn pattern_host(host: &str) -> Option<String> {
    if host == "all" {
        return Some(String::new());
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    Url::parse(&format!("http://{host}/"))
        .ok()?
        .host_str()
        .map(str::to_owned)
}

/// `boostHostFor`: the host a new Boost for this page is named after, without `www.`.
pub fn boost_host_for(url: Option<&str>) -> Option<String> {
    let url = url.filter(|url| starts_with_http(url))?;
    let hostname = hostname(url)?;
    let host = hostname.strip_prefix("www.").unwrap_or(&hostname);
    (!host.is_empty()).then(|| host.to_owned())
}

fn is_web_page(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// `/^https?:/.test(url)`
fn starts_with_http(url: &str) -> bool {
    url.starts_with("http:") || url.starts_with("https:")
}

/// `new URL(url).hostname`, which is empty for a URL with no host.
fn hostname(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    Some(url.host_str().unwrap_or_default().to_owned())
}
