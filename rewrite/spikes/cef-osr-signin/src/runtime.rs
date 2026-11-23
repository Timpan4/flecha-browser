//! Checks, before CEF starts, that the libcef.so this process loaded is the pinned runtime and
//! that its resources are next to it. A missing libcef.so fails in the dynamic loader before
//! `main`; run.sh reports that case.

use cef::{api_hash, api_version, sys};
use serde_json::{Value, json};
use std::{
    ffi::CStr,
    fs,
    path::{Path, PathBuf},
};

/// From rewrite/pins.toml, compiled in by build.rs.
const PIN_VERSION: &str = env!("FLECHA_PIN_CEF_VERSION");
const PIN_CHANNEL: &str = env!("FLECHA_PIN_CEF_CHANNEL");
const PIN_SHA1: &str = env!("FLECHA_PIN_CEF_SHA1");

/// Files CEF reads at startup that the minimal archive ships next to libcef.so ("Release" and
/// "Resources" in the archive's README.txt). `en-US.pak` is the locale CEF falls back to.
const REQUIRED: &[&str] = &[
    "icudtl.dat",
    "v8_context_snapshot.bin",
    "resources.pak",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
    "locales/en-US.pak",
];

pub struct Runtime {
    pub dir: PathBuf,
    pub libcef: PathBuf,
    pub commit: String,
    pub archive: Value,
}

impl Runtime {
    pub fn report(&self) -> Value {
        json!({
            "pinned_cef_version": PIN_VERSION,
            "pinned_channel": PIN_CHANNEL,
            "pinned_sha1": PIN_SHA1,
            "compiled_cef_version": compiled_version(),
            "runtime_api_version": api_version(),
            "runtime_cef_commit": self.commit,
            "loaded_libcef": self.libcef,
            "archive": self.archive,
        })
    }
}

fn compiled_version() -> String {
    String::from_utf8_lossy(sys::CEF_VERSION)
        .trim_end_matches('\0')
        .to_string()
}

fn hash(entry: i32) -> Option<String> {
    let ptr = api_hash(sys::CEF_API_VERSION_LAST, entry);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null; cef_api_hash returns a NUL-terminated string owned by libcef.
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn loaded_libcef() -> Result<PathBuf, String> {
    let maps = fs::read_to_string("/proc/self/maps")
        .map_err(|e| format!("cannot read /proc/self/maps: {e}"))?;
    maps.lines()
        .filter_map(|l| l.split_whitespace().nth(5))
        .find(|p| p.ends_with("/libcef.so"))
        .map(PathBuf::from)
        .ok_or_else(|| "libcef.so is not mapped into this process".to_string())
}

/// Configures the API version and checks the loaded runtime. Call once, before any other CEF
/// function, in every process.
pub fn configure_api() -> Result<(), String> {
    if hash(0).is_none() {
        return Err("cef_api_hash returned null".into());
    }
    let loaded = api_version();
    if loaded != sys::CEF_API_VERSION_LAST {
        return Err(format!(
            "libcef runs API version {loaded}, this build needs {}",
            sys::CEF_API_VERSION_LAST
        ));
    }
    Ok(())
}

/// The browser-process check: pin, commit, archive and resources.
pub fn check() -> Result<Runtime, String> {
    if compiled_version() != PIN_VERSION {
        return Err(format!(
            "the cef-rs bindings are for CEF {}, pins.toml pins {PIN_VERSION}",
            compiled_version()
        ));
    }
    let libcef = loaded_libcef()?;
    let dir = libcef
        .parent()
        .ok_or("libcef.so has no parent directory")?
        .to_path_buf();

    let commit = hash(2).ok_or("cef_api_hash(…, 2) returned null")?;
    let short = PIN_VERSION
        .split('+')
        .find_map(|part| part.strip_prefix('g'))
        .ok_or("pinned version has no +g<commit> part")?;
    if !commit.starts_with(short) {
        return Err(format!(
            "{} is CEF commit {commit}, the pin is {short}",
            libcef.display()
        ));
    }

    let archive_path = dir.join("archive.json");
    let archive: Value = fs::read_to_string(&archive_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        .map_err(|e| format!("cannot read {}: {e}", archive_path.display()))?;
    let sha1 = archive.get("sha1").and_then(Value::as_str);
    if sha1 != Some(PIN_SHA1) {
        return Err(format!(
            "{} names SHA-1 {}, the {PIN_CHANNEL} pin is {PIN_SHA1}",
            archive_path.display(),
            sha1.unwrap_or("(none)")
        ));
    }

    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|f| !Path::new(&dir).join(f).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "CEF resources missing in {}: {}",
            dir.display(),
            missing.join(", ")
        ));
    }
    Ok(Runtime {
        dir,
        libcef,
        commit,
        archive,
    })
}
