use std::fmt;

use url::{Host, Url};

/// A web origin, `scheme://host[:port]`, that can hold site permissions.
///
/// Only `http` and `https` URLs have one. Opaque origins (`data:`, `file:`, `javascript:`,
/// sandboxed frames) and every other scheme are rejected, so they can never be granted anything.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    serialized: String,
    trustworthy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginError {
    Invalid,
    UnsupportedScheme(String),
    Opaque,
    /// A saved origin that does not serialize back to itself.
    NotCanonical,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => f.write_str("not a URL"),
            Self::UnsupportedScheme(scheme) => write!(f, "{scheme}: URLs have no site permissions"),
            Self::Opaque => f.write_str("opaque origin"),
            Self::NotCanonical => f.write_str("origin is not in canonical form"),
        }
    }
}

impl std::error::Error for OriginError {}

impl Origin {
    /// The origin of a page URL. Path, query, fragment and credentials are dropped, the host is
    /// lowercased (and punycoded), and the scheme's default port is omitted.
    pub fn parse(url: &str) -> Result<Self, OriginError> {
        let url = Url::parse(url).map_err(|_| OriginError::Invalid)?;
        // `Url::origin` unwraps `blob:` and gives tuple origins to `ws:` and `ftp:`.
        let secure = match url.scheme() {
            "https" => true,
            "http" => false,
            other => return Err(OriginError::UnsupportedScheme(other.to_owned())),
        };
        let origin = url.origin();
        if !origin.is_tuple() {
            return Err(OriginError::Opaque);
        }
        Ok(Self {
            serialized: origin.ascii_serialization(),
            trustworthy: secure || url.host().is_some_and(is_loopback),
        })
    }

    /// Parses a saved origin, which must already be in canonical form.
    pub(crate) fn parse_canonical(serialized: &str) -> Result<Self, OriginError> {
        let origin = Self::parse(serialized)?;
        if origin.serialized != serialized {
            return Err(OriginError::NotCanonical);
        }
        Ok(origin)
    }

    pub fn as_str(&self) -> &str {
        &self.serialized
    }

    /// Chromium's "potentially trustworthy" test for secure contexts: `https`, or `http` on a
    /// loopback host (`localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`).
    pub fn is_potentially_trustworthy(&self) -> bool {
        self.trustworthy
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.serialized)
    }
}

fn is_loopback(host: Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.strip_suffix('.').unwrap_or(name);
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}
