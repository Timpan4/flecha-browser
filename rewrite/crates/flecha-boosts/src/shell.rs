//! Boosts saved by the Chromium shell, for the profile migration.

use serde_json::Value;

use crate::name::BoostName;
use crate::snapshot::compiled_boost;
use crate::store::{Boosts, Entry};

/// The shell's Boosts keys in `chrome.storage.local`.
pub const SHELL_FILES_KEY: &str = "boosts";
pub const SHELL_BUILDS_KEY: &str = "boostsCompiled";

/// The shell's stored Boosts were not objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidShellShape;

impl std::fmt::Display for InvalidShellShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the shell's saved Boosts are not an object")
    }
}

impl std::error::Error for InvalidShellShape {}

#[derive(Debug)]
pub struct ShellImport {
    pub boosts: Boosts,
    /// Keys of `boosts` the shell never used: not a Boost name, or code that isn't a string.
    pub skipped: Vec<String>,
}

impl Boosts {
    /// Read the shell's `boosts` (name to code) and `boostsCompiled` (name to build) values.
    ///
    /// Key order is kept, since Boosts apply in that order. Like the shell, a missing value is
    /// empty, and a build without a matching Boost is ignored. Parse the stored JSON with
    /// serde_json's `preserve_order` feature, which this crate enables.
    pub fn from_shell(
        files: Option<&Value>,
        builds: Option<&Value>,
    ) -> Result<ShellImport, InvalidShellShape> {
        let empty = serde_json::Map::new();
        let files = object(files, &empty)?;
        let builds = object(builds, &empty)?;
        let mut import = ShellImport {
            boosts: Boosts::new(),
            skipped: Vec::new(),
        };
        for (key, code) in files {
            let (Some(name), Some(code)) = (BoostName::parse(key), code.as_str()) else {
                import.skipped.push(key.clone());
                continue;
            };
            let compiled = builds.get(key).and_then(compiled_boost);
            import.boosts.entries.insert(
                name,
                Entry {
                    code: code.to_owned(),
                    compiled,
                },
            );
        }
        Ok(import)
    }
}

/// `saved[key] ?? {}`, for a value that is an object when present.
fn object<'a>(
    value: Option<&'a Value>,
    empty: &'a serde_json::Map<String, Value>,
) -> Result<&'a serde_json::Map<String, Value>, InvalidShellShape> {
    match value {
        None | Some(Value::Null) => Ok(empty),
        Some(Value::Object(object)) => Ok(object),
        Some(_) => Err(InvalidShellShape),
    }
}
