//! Element hiding for one page, made safe to join into a stylesheet. The selector checks are
//! ported from `buildAdblock` in `chromium/shell/build-adblock.mjs`.

use std::collections::HashSet;

/// What a page should hide.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CosmeticFilters {
    /// Selectors to hide on this page, sorted: the site's own and, unless `generic_hide` is
    /// set, the generic ones that aren't plain class or id selectors.
    pub hide_selectors: Vec<String>,
    /// Class and id selectors the page excepts from generic hiding. Pass them back to
    /// [`crate::AdBlocker::hidden_class_id_selectors`].
    pub exceptions: HashSet<String>,
    /// No generic hiding on this page: a list exempts it, blocking is off, or the site is
    /// allowed. Then there's no need to report the page's classes and ids.
    pub generic_hide: bool,
}

impl CosmeticFilters {
    /// Nothing hidden and no generic hiding.
    pub fn none() -> Self {
        Self {
            generic_hide: true,
            ..Self::default()
        }
    }

    /// One rule per selector, as build-adblock.mjs writes cosmetic.css: a selector Blink doesn't
    /// know drops only its own rule.
    pub fn stylesheet(&self) -> String {
        stylesheet(&self.hide_selectors)
    }
}

pub fn stylesheet(selectors: &[String]) -> String {
    selectors
        .iter()
        .map(|selector| {
            // A trailing backslash must not escape the opening brace and consume the next rule.
            let pad = if selector.ends_with('\\') { " " } else { "" };
            format!("{selector}{pad}{{display:none!important}}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a selector can go into a stylesheet without consuming the rules after it: no braces,
/// and no unclosed string, comment, attribute or function.
pub fn is_injectable_selector(selector: &str) -> bool {
    !selector.contains(['{', '}']) && has_balanced_syntax(selector)
}

/// Every character it looks at is ASCII, so it walks bytes. Skipping one byte after a backslash
/// can land inside a multibyte character, whose remaining bytes are never ASCII; JavaScript
/// skipping one UTF-16 unit of a surrogate pair ends the same way.
fn has_balanced_syntax(selector: &str) -> bool {
    let bytes = selector.as_bytes();
    let mut quote = None;
    let mut closers = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\\' {
            // Escaped characters do not affect selector structure.
            i += 1;
        } else if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let Some(end) = selector.get(i + 2..).and_then(|rest| rest.find("*/")) else {
                return false;
            };
            i += 2 + end + 1;
        } else if byte == b'[' || byte == b'(' {
            closers.push(if byte == b'[' { b']' } else { b')' });
        } else if (byte == b']' || byte == b')') && closers.pop() != Some(byte) {
            return false;
        }
        i += 1;
    }
    quote.is_none() && closers.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_selectors_cannot_consume_the_next_rule() {
        // From build-adblock.test.mjs.
        let escaped = r#":is([data-ad="escaped\" quote ] )"], .ad\[\()/* unmatched ' [ ( */"#;
        assert!(is_injectable_selector(escaped));
        for bad in [
            r#"[data-ad="unterminated]"#,
            ".ad/* unterminated",
            "[data-ad=test",
            ":is(.ad",
            ".ad]",
            ".ad)",
            ":is([data-ad=test)]",
            ".ad { color: red }",
            ".ad}",
        ] {
            assert!(!is_injectable_selector(bad), "{bad}");
        }
        assert!(is_injectable_selector(".next-ad"));
        assert!(is_injectable_selector("/**/.ad"));
        assert!(!is_injectable_selector("/*/.ad"));
    }

    #[test]
    fn stylesheet_has_one_rule_per_selector() {
        let selectors = [".ad".to_owned(), r".odd\".to_owned()];
        assert_eq!(
            stylesheet(&selectors),
            ".ad{display:none!important}\n.odd\\ {display:none!important}"
        );
    }
}
