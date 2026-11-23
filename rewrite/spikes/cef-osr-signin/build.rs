//! Compiles the CEF pin from rewrite/pins.toml into the binary, so `main` can refuse to run on
//! any other runtime. The runtime itself is staged by prepare.sh and found through run.sh;
//! this script has no side effects outside OUT_DIR.

use std::{env, fs, path::PathBuf};

fn main() {
    let pins = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("../../pins.toml");
    println!("cargo::rerun-if-changed={}", pins.display());
    let text =
        fs::read_to_string(&pins).unwrap_or_else(|e| panic!("cannot read {}: {e}", pins.display()));

    let channel = pin(&text, "cef", "channel");
    let sha1 = pin(&text, &format!("cef.{channel}.sha1"), "linux64");
    println!(
        "cargo::rustc-env=FLECHA_PIN_CEF_VERSION={}",
        pin(&text, "cef", "version")
    );
    println!("cargo::rustc-env=FLECHA_PIN_CEF_CHANNEL={channel}");
    println!("cargo::rustc-env=FLECHA_PIN_CEF_SHA1={sha1}");
}

/// Reads `key = "value"` from `[table]`, the same subset of TOML that prepare.sh reads.
fn pin(text: &str, table: &str, key: &str) -> String {
    let mut active = "";
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            active = name;
        } else if active == table
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let v = v.trim();
            if let Some(value) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                return value.to_string();
            }
        }
    }
    panic!("missing pin {table}.{key} in pins.toml");
}
