//! The parts of the spike that need neither CEF nor GPUI, so `cargo test` can
//! run them without loading libcef.

pub mod schedule;

/// Template tokens left in a document: `@NAME@`, or `NNNNAMENNN`, the shape a
/// botched fill leaves when it replaces each `@` with the same number.
pub fn template_tokens(text: &str) -> Vec<String> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let is_name = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };
    let mut found = Vec::new();
    for (at, _) in text.match_indices('@') {
        let rest = &text[at + 1..];
        let name_len = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
        let name = &rest[..name_len];
        if rest[name_len..].starts_with('@') && is_name(name) {
            found.push(format!("@{name}@"));
        }
    }
    for word in text.split(|c: char| !is_word(c)) {
        // The same number on both ends, around a NAME (which may hold digits).
        let lead = word.len() - word.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if lead > 0 && word.len() > 2 * lead && word.ends_with(&word[..lead]) {
            let middle = &word[lead..word.len() - lead];
            if is_name(middle) {
                found.push(word.to_string());
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::template_tokens;

    #[test]
    fn report_has_no_template_tokens() {
        let tokens = template_tokens(include_str!("../REPORT.md"));
        assert!(
            tokens.is_empty(),
            "REPORT.md has unfilled tokens: {tokens:?}"
        );
    }

    #[test]
    fn template_tokens_finds_both_shapes_and_skips_real_text() {
        assert_eq!(template_tokens("a @IDLE_CPU@ b"), ["@IDLE_CPU@"]);
        assert_eq!(
            template_tokens("in 966IDLE_CALLS966 calls"),
            ["966IDLE_CALLS966"]
        );
        assert_eq!(template_tokens("966GAP50966"), ["966GAP50966"]);
        assert_eq!(
            template_tokens("CSS @keyframes then @HOLD_MS@ ms"),
            ["@HOLD_MS@"]
        );
        let real = "CSS @keyframes, CEF 158.0.3+g8c29be2, XDG_RUNTIME_DIR, 1024 x 640, x11, \
                    email@example.com, `CEF_RUNTIME_STYLE_CHROME`, 3995 to 4007 ms";
        assert!(
            template_tokens(real).is_empty(),
            "{:?}",
            template_tokens(real)
        );
    }
}
