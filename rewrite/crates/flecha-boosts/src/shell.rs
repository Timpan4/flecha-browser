//! Boosts saved by the Chromium shell, for the profile migration.

use serde_json::{Map, Value};

use crate::name::BoostName;
use crate::snapshot::compiled_boost;
use crate::store::{Boosts, Entry};

/// The shell's Boosts keys in `chrome.storage.local`.
pub const SHELL_FILES_KEY: &str = "boosts";
pub const SHELL_BUILDS_KEY: &str = "boostsCompiled";

#[derive(Debug)]
pub struct ShellImport {
    pub boosts: Boosts,
    /// Keys of `boosts` the shell never used: not a Boost name, or code that isn't a string.
    pub skipped: Vec<String>,
    /// Boosts kept without their build, because it wasn't an object of strings. A script in
    /// this list runs nothing until it is saved again.
    pub dropped_builds: Vec<String>,
    /// [`SHELL_FILES_KEY`] or [`SHELL_BUILDS_KEY`] when its value wasn't an object. The shell
    /// found no Boost, or no build, in such a value, and neither does the import.
    pub unreadable: Vec<&'static str>,
}

impl Boosts {
    /// Read the shell's `boosts` (name to code) and `boostsCompiled` (name to build) values.
    ///
    /// Key order is kept, since Boosts apply in that order. Like the shell, a missing value is
    /// empty, and a build without a matching Boost is ignored. Parse the stored JSON with
    /// serde_json's `preserve_order` feature, which this crate enables.
    pub fn from_shell(files: Option<&Value>, builds: Option<&Value>) -> ShellImport {
        let mut import = ShellImport {
            boosts: Boosts::new(),
            skipped: Vec::new(),
            dropped_builds: Vec::new(),
            unreadable: Vec::new(),
        };
        let empty = Map::new();
        let files = object(files, &empty, SHELL_FILES_KEY, &mut import.unreadable);
        let builds = object(builds, &empty, SHELL_BUILDS_KEY, &mut import.unreadable);
        for (key, code) in files {
            let (Some(name), Some(code)) = (BoostName::parse(key), code.as_str()) else {
                import.skipped.push(key.clone());
                continue;
            };
            let compiled = builds.get(key).and_then(|build| {
                let compiled = compiled_boost(build);
                if compiled.is_none() {
                    import.dropped_builds.push(key.clone());
                }
                compiled
            });
            import.boosts.entries.insert(
                name,
                Entry {
                    code: code.to_owned(),
                    compiled,
                },
            );
        }
        import
    }
}

/// `saved[key] ?? {}`. The shell indexed any other value by Boost name and found nothing there:
/// a string's or array's keys are indexes, and a number or boolean has none.
fn object<'a>(
    value: Option<&'a Value>,
    empty: &'a Map<String, Value>,
    key: &'static str,
    unreadable: &mut Vec<&'static str>,
) -> &'a Map<String, Value> {
    match value {
        None | Some(Value::Null) => empty,
        Some(Value::Object(object)) => object,
        Some(_) => {
            unreadable.push(key);
            empty
        }
    }
}
