use flecha_core::SearchEngine;
use flecha_core::suggestions::{InputResolution, resolve_input, suggestable};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddressCase {
    input: String,
    ts_url: String,
    ts_suggestable: bool,
    rust: Option<Divergence>,
}

#[derive(Deserialize)]
struct Divergence {
    reason: String,
    url: Option<String>,
    suggestable: Option<bool>,
}

#[test]
fn command_bar_matches_the_saved_chromium_corpus() {
    for line in include_str!("fixtures/address-parity.jsonl").lines() {
        let case: AddressCase = serde_json::from_str(line).unwrap();
        if let Some(divergence) = &case.rust {
            assert!(
                !divergence.reason.is_empty()
                    && (divergence
                        .url
                        .as_ref()
                        .is_some_and(|url| *url != case.ts_url)
                        || divergence
                            .suggestable
                            .is_some_and(|value| value != case.ts_suggestable)),
                "stale or undocumented divergence for {:?}",
                case.input,
            );
        }
        let expect_url = case
            .rust
            .as_ref()
            .and_then(|d| d.url.as_deref())
            .unwrap_or(&case.ts_url);
        let expect_suggestable = case
            .rust
            .as_ref()
            .and_then(|d| d.suggestable)
            .unwrap_or(case.ts_suggestable);
        let expected = if expect_url == "about:blank" {
            InputResolution::Invalid
        } else if expect_url.starts_with("https://www.google.com/search?q=") {
            InputResolution::Search(expect_url.into())
        } else {
            InputResolution::Navigate(expect_url.into())
        };
        assert_eq!(
            resolve_input(&case.input, SearchEngine::Google),
            expected,
            "input {:?}",
            case.input
        );
        assert_eq!(
            suggestable(&case.input),
            expect_suggestable,
            "suggestable({:?})",
            case.input
        );
    }
}
