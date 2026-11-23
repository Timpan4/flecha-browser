//! Which pages a Boost applies to, from the shell's `lib/boostFiles.ts`.

use url::Url;

/// `boostMatches`: whether a Boost for `host` applies to the page at `url`.
///
/// `all` takes any URL that starts with `http:` or `https:`, compared as written. Any other host
/// is compared with the URL's WHATWG hostname: equal, or a subdomain of it. The stored host is
/// compared as stored, so an uppercase or Unicode host never matches. Ports and user info play
/// no part. This takes any scheme. Use [`applies`] for what the shell actually styled or ran.
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

/// Whether the shell styled or ran a Boost for `host` on the page at `url`.
///
/// The shell's manifest asks for `<all_urls>`, and `boostMatches` takes any scheme with a host,
/// such as `chrome://settings` for a Boost named `settings.css`. Chrome's scripting and user
/// script APIs refused every page that isn't http or https: internal, extension and `file:` pages
/// (the shell has no file access). CEF has no such layer, so the rewrite checks the scheme here.
pub fn applies(url: &str, host: &str) -> bool {
    is_web_page(url) && boost_matches(url, host)
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
