//! Boost names, from the shell's `lib/boostFiles.ts`.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BoostKind {
    /// TypeScript, `<host>.ts`.
    Script,
    /// CSS, `<host>.css`.
    Styles,
}

impl BoostKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Script => ".ts",
            Self::Styles => ".css",
        }
    }
}

/// A Boost's name: `github.com.ts`, `github.com.css` or `all.css`.
///
/// It follows `parseBoostName`, so a stored name may carry any host text except a JavaScript
/// line terminator. That keeps the name safe in the one-line `sourceURL` comment of a compiled
/// Boost. Saving asks more of the host (see `Boosts::save`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BoostName {
    name: String,
    kind: BoostKind,
}

impl BoostName {
    /// `parseBoostName`: `/^(.+)\.(ts|css)$/`, with a host that doesn't start with a dot.
    pub fn parse(name: &str) -> Option<Self> {
        let (host, kind) = if let Some(host) = name.strip_suffix(".ts") {
            (host, BoostKind::Script)
        } else {
            (name.strip_suffix(".css")?, BoostKind::Styles)
        };
        // JavaScript's `.` matches anything but a line terminator.
        let valid =
            !host.is_empty() && !host.starts_with('.') && !host.contains(is_line_terminator);
        valid.then(|| Self {
            name: name.to_owned(),
            kind,
        })
    }

    /// `boostName`: the host followed by the kind's extension.
    pub fn for_host(host: &str, kind: BoostKind) -> Option<Self> {
        Self::parse(&format!("{host}{}", kind.extension()))
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }

    pub fn host(&self) -> &str {
        &self.name[..self.name.len() - self.kind.extension().len()]
    }

    pub fn kind(&self) -> BoostKind {
        self.kind
    }
}

impl fmt::Display for BoostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl Serialize for BoostName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name)
    }
}

/// ECMAScript LineTerminator.
pub(crate) fn is_line_terminator(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_never_holds_a_line_terminator() {
        for name in ["a\nb.ts", "a\rb.ts", "a\u{2028}b.ts", "a\u{2029}b.css"] {
            assert_eq!(BoostName::parse(name), None, "{name:?}");
        }
        assert!(BoostName::parse("a\u{85}b.ts").is_some());
    }
}
