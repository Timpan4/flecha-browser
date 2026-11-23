#[test]
fn chromium_facing_inputs_use_ecmascript_whitespace() {
    let forbidden = regex::Regex::new(
        r"(?:\\[sS]|(?:\.|::)\s*(?:trim|trim_start|trim_end|trim_ascii(?:_start|_end)?|split_(?:ascii_)?whitespace|is_whitespace)\b)",
    ).unwrap();
    for (name, source) in [
        ("input_text.rs", include_str!("../src/input_text.rs")),
        ("suggestions.rs", include_str!("../src/suggestions.rs")),
        (
            "suggestions/address.rs",
            include_str!("../src/suggestions/address.rs"),
        ),
        ("state.rs", include_str!("../src/state.rs")),
        ("import.rs", include_str!("../src/import.rs")),
        ("storage.rs", include_str!("../src/storage.rs")),
        (
            "storage/snapshot.rs",
            include_str!("../src/storage/snapshot.rs"),
        ),
    ] {
        for (line, text) in source.lines().enumerate() {
            assert!(
                !forbidden.is_match(text),
                "{name}:{} must use the shared ECMAScript whitespace rules: {text}",
                line + 1,
            );
        }
    }
}
