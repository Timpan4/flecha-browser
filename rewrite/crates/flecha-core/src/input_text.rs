//! ECMAScript WhiteSpace and LineTerminator for text shared with the Chromium shell.

macro_rules! define_js_whitespace {
    ($($low:literal..=$high:literal),+; $($single:literal),+ $(,)?) => {
        pub(crate) fn is_js_whitespace(ch: char) -> bool {
            matches!(ch, $($low..=$high)|+ $(| $single)+)
        }

        // The contents of a regex character class, usable in negated classes too.
        macro_rules! js_space_chars {
            () => { concat!($($low, "-", $high,)+ $($single),+) };
        }
    };
}

define_js_whitespace!(
    '\u{0009}'..='\u{000D}', '\u{2000}'..='\u{200A}';
    ' ', '\u{00A0}', '\u{1680}', '\u{2028}', '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}',
);

pub(crate) fn trim_input(input: &str) -> &str {
    input.trim_matches(is_js_whitespace)
}

pub(crate) fn split_input(input: &str) -> impl Iterator<Item = &str> {
    input
        .split(is_js_whitespace)
        .filter(|word| !word.is_empty())
}
