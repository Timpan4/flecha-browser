use url::{Host, Url};

use super::{decode_component, default_ignorables, encode_component};
use crate::SearchEngine;
use crate::input_text::{is_js_whitespace, trim_input};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputResolution {
    Navigate(String),
    Search(String),
    Invalid,
}

pub fn resolve_input(raw: &str, engine: SearchEngine) -> InputResolution {
    let input = trim_input(raw);
    if let Some(address) = web_address(input) {
        return InputResolution::Navigate(address);
    }
    if input.is_empty() || rejected_address(input) {
        return InputResolution::Invalid;
    }
    InputResolution::Search(engine.search_url(input))
}

pub fn web_address(raw: &str) -> Option<String> {
    address(raw, false)
}

pub fn pinnable_address(raw: &str) -> Option<String> {
    let url = Url::parse(&web_address(raw)?).ok()?;
    (matches!(url.scheme(), "http" | "https") || url.as_str() == "about:blank").then(|| url.into())
}

fn has_credentials(input: &str) -> bool {
    let input = trim_input(input);
    let Some((userinfo, destination)) = input.split_once('@') else {
        return false;
    };
    if userinfo.is_empty() || destination.starts_with(is_js_whitespace) {
        return false;
    }
    let has_tail = re!(r"^[^", js_space_chars!(), r"/?#\\]+[/?#\\]").is_match(destination);
    match userinfo.split_once(':') {
        Some((name, password)) if !name.contains(['/', '?', '#']) => {
            has_tail
                || !password.starts_with(is_js_whitespace)
                || trim_input(password).contains(is_js_whitespace)
        }
        None => {
            !userinfo.contains(['/', '?', '#'])
                && (has_tail || !userinfo.contains(is_js_whitespace))
        }
        _ => false,
    }
}

pub fn rejected_address(raw: &str) -> bool {
    if has_credentials(raw)
        || trim_input(raw)
            .split_once(':')
            .is_some_and(|(scheme, tail)| {
                // Look through invisible scheme characters for privacy. Navigation still
                // uses the original input and must pass its own validation.
                let visible = default_ignorables().replace_all(scheme, "");
                let visible = visible.replace(char::is_control, "");
                is_http_scheme(&visible.replace(is_js_whitespace, ""))
                    || (tail.starts_with("//")
                        && !visible.is_empty()
                        && !visible.contains(is_js_whitespace))
            })
    {
        return true;
    }
    let input = raw.replace('？', "?");
    let split = input.find(['/', '?', '#', '\\']).unwrap_or(input.len());
    let (typed, tail) = input.split_at(split);
    let decoded = decode_component(typed);
    let authority =
        re!(r"[。．｡]|(?i-u:%2e|%e3%80%82|%ef%bc%8e|%ef%bd%a1)").replace_all(&decoded, ".");
    let authority = trim_input(&authority);
    if has_credentials(&format!("{authority}{tail}"))
        || re!(
            r"^\[[^",
            js_space_chars!(),
            r"\[\]]*:[^",
            js_space_chars!(),
            r"\[\]]*(?:\]|$)"
        )
        .is_match(authority)
        || (re!(r"(?i-u:^[0-9a-f:.]+(?:%[a-z0-9_.-]+)?$)").is_match(authority)
            && (authority.contains("::") || authority.matches(':').count() >= 3))
    {
        return true;
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| {
            (trim_input(host), Some(trim_input(port)))
        });
    let address_tail =
        re!(r"^[/\\?#][^", js_space_chars!(), r"]").is_match(tail) || tail.contains('=');
    let numeric_port = port
        .is_some_and(|port| re!(r"^[+-]?[0-9]").is_match(port) && !port.contains(is_js_whitespace));
    if re!(
        r"^(?i-u:localhost)$|^[^",
        js_space_chars!(),
        r".@]+\.[^",
        js_space_chars!(),
        r"@]+$"
    )
    .is_match(host)
    {
        if port.is_some() {
            return !typed.contains(is_js_whitespace) || address_tail || numeric_port;
        }
        let separated_question = typed.ends_with(is_js_whitespace)
            && re!(
                r"^\?[",
                js_space_chars!(),
                r"]+(?:[^",
                js_space_chars!(),
                r"]+[",
                js_space_chars!(),
                r"]+[^",
                js_space_chars!(),
                r"]|(?i-u:who|what|when|where|why|which|how)\?$)"
            )
            .is_match(tail)
            && !tail.contains('=');
        return !tail.is_empty() && !separated_question;
    }
    port.is_some()
        && !tail.is_empty()
        && (address_tail || numeric_port)
        && Host::parse(host).is_ok_and(|host| {
            matches!(host, Host::Domain(domain) if re!(r"(?i-u:^[a-z0-9-]*[a-z][a-z0-9-]*$)").is_match(&domain))
        })
}

fn is_http_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

pub(super) fn address(raw: &str, allow_bare_host: bool) -> Option<String> {
    let input = trim_input(raw);
    if re!(r"^(?i-u:about|data):").is_match(input) {
        return Some(escape_controls(input));
    }
    if let Some(authority) = re!(r"^(?i-u:[a-z][a-z0-9+.-]*)://[^/?#\\]*").find(input) {
        let authority = authority.as_str();
        let clean = authority.replace(['\t', '\r', '\n'], "");
        let (scheme, endpoint) = clean.split_once("://")?;
        let tail = escape_controls(&input[authority.len()..]);
        let address = format!("{clean}{tail}");
        match scheme.to_ascii_lowercase().as_str() {
            // Reject an empty authority before parsing. The URL parser can
            // otherwise interpret the first path segment as the host.
            "http" | "https" | "ftp" | "ws" | "wss" => {
                if endpoint.is_empty() {
                    return None;
                }
                Url::parse(&address).ok()?;
            }
            "file" => {
                Url::parse(&address).ok()?;
            }
            _ => {
                // The scheme syntax was checked above. All non-special schemes use
                // the same opaque-host rules.
                Url::parse(&format!("app://{endpoint}{tail}")).ok()?;
            }
        }
        return Some(address);
    }
    if re!(r"^(?i-u:file):").is_match(input) {
        let address = escape_controls(input);
        Url::parse(&address).ok()?;
        return Some(address);
    }
    if re!(r"^(?i-u:mailto):").is_match(input) {
        let recipient = input[7..].split('?').next().unwrap_or_default();
        return (!recipient.contains(is_js_whitespace)).then(|| input.into());
    }
    let end = input.find(['/', '?', '#']).unwrap_or(input.len());
    let (authority, tail) = input.split_at(end);
    let bare = re!(
        r"^((?i-u:\[[0-9a-f:.]+\])|[^:",
        js_space_chars!(),
        r"\\@\[\]]+)(?::([0-9]+))?$"
    )
    .captures(authority)?;
    let host = &bare[1];
    if bare.get(2).is_some()
        && re!(r"^(?i-u:blob|ftp|http|https|news|sms|ssh|tel|urn)$").is_match(host)
    {
        return None;
    }
    let endpoint = format!("{authority}{}", escape_controls(tail));
    let parsed = Url::parse(&format!("https://{endpoint}")).ok()?;
    let local = match parsed.host()? {
        Host::Ipv4(address) => {
            // WHATWG permits numeric shorthand and octal IPv4; Flecha accepts canonical quads.
            if address.to_string() != host.strip_suffix('.').unwrap_or(host) {
                return None;
            }
            address.is_loopback() || address.is_private() || address.is_unspecified()
        }
        Host::Ipv6(address) => address.is_loopback(),
        Host::Domain(domain) => {
            let domain = domain.strip_suffix('.').unwrap_or(domain);
            let local = re!(r"(?i-u:^(?:[a-z0-9.-]+\.)?localhost$)").is_match(domain);
            let idn = re!(r"(?i-u:\.xn--[a-z0-9-]+$)").is_match(domain)
                && re!(r"[.。．｡][\p{L}\p{M}\p{N}-]+[.。．｡]?$").is_match(host);
            if !local && !idn && !re!(r"(?i-u:\.[a-z]{2,}$)").is_match(domain) && !allow_bare_host {
                return None;
            }
            local
        }
    };
    let scheme = if local { "http" } else { "https" };
    let result = format!("{scheme}://{endpoint}");
    if input.contains(is_js_whitespace) {
        Url::parse(&result).ok().map(Into::into)
    } else {
        Some(result)
    }
}

fn escape_controls(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\t' | '\n' | '\r') {
            result.push_str(&encode_component(character.encode_utf8(&mut [0; 4])));
        } else {
            result.push(character);
        }
    }
    result
}
