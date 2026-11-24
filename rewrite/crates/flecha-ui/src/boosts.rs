use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, ensure};
use flecha_boosts::{BoostKind, BoostName, Boosts, CompiledBoost, compile, snapshot};

use crate::Controller;

const CURRENT: &str = "boosts.json";

impl Controller {
    pub(crate) fn edit_boosts<T>(
        &mut self,
        edit: impl FnOnce(&mut ProfileBoosts) -> Result<T>,
    ) -> Result<T> {
        let revision = self.boosts.revision();
        let result = edit(&mut self.boosts)?;
        if revision != self.boosts.revision() {
            self.engine.set_boosts(self.boosts().clone());
            self.engine.reload_boosts();
        }
        Ok(result)
    }
}

#[derive(Default, Eq, PartialEq)]
struct SavedBoosts {
    all: Boosts,
    disabled: BTreeSet<String>,
}

pub(crate) struct ProfileBoosts {
    directory: PathBuf,
    saved: SavedBoosts,
    enabled: Boosts,
    revision: u64,
    load_error: Option<anyhow::Error>,
}

impl ProfileBoosts {
    pub(crate) fn open(directory: PathBuf) -> Self {
        let mut profile = Self {
            directory,
            saved: SavedBoosts::default(),
            enabled: Boosts::new(),
            revision: 0,
            load_error: None,
        };
        match profile.read() {
            Ok(saved) => profile.install(saved),
            Err(error) => profile.load_error = Some(error),
        }
        profile
    }

    pub(crate) fn all(&self) -> &Boosts {
        &self.saved.all
    }

    pub(crate) fn enabled(&self) -> &Boosts {
        &self.enabled
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn load_error(&self) -> Option<&anyhow::Error> {
        self.load_error.as_ref()
    }

    pub(crate) fn create(&mut self, host: &str, kind: BoostKind) -> Result<BoostName> {
        self.edit(|saved| {
            saved
                .all
                .create(host, kind)
                .map(|(name, _)| name)
                .context("Enter a hostname such as example.com, or all")
        })
    }

    pub(crate) fn save(
        &mut self,
        name: &BoostName,
        source: &str,
        build: Option<CompiledBoost>,
    ) -> Result<()> {
        self.edit(|saved| {
            ensure!(
                saved.all.get(name.as_str()).is_some(),
                "This Boost was deleted"
            );
            saved
                .all
                .save(name.as_str(), source, build, false)
                .context("This Boost’s filename cannot be edited")?;
            Ok(())
        })
    }

    pub(crate) fn set_enabled(&mut self, name: &BoostName, enabled: bool) -> Result<()> {
        self.edit(|saved| {
            ensure!(
                saved.all.get(name.as_str()).is_some(),
                "This Boost was deleted"
            );
            if enabled {
                saved.disabled.remove(name.as_str());
            } else {
                saved.disabled.insert(name.to_string());
            }
            Ok(())
        })
    }

    pub(crate) fn delete(&mut self, name: &BoostName) -> Result<()> {
        self.edit(|saved| {
            saved.all.remove(name.as_str());
            saved.disabled.remove(name.as_str());
            Ok(())
        })
    }

    fn edit<T>(&mut self, edit: impl FnOnce(&mut SavedBoosts) -> Result<T>) -> Result<T> {
        ensure!(
            self.load_error.is_none(),
            "Boosts are read-only after a load error"
        );
        let mut next = SavedBoosts {
            all: self.saved.all.clone(),
            disabled: self.saved.disabled.clone(),
        };
        let result = edit(&mut next)?;
        if next != self.saved {
            ensure!(
                self.read().context("Check existing Boosts before saving")? == self.saved,
                "Boosts changed on disk. Restart Flecha to load the current file"
            );
            self.write(&next).context("Save Boosts")?;
            self.install(next);
            self.revision += 1;
        }
        Ok(result)
    }

    fn install(&mut self, saved: SavedBoosts) {
        self.enabled = saved.all.clone();
        for name in &saved.disabled {
            self.enabled.remove(name);
        }
        self.saved = saved;
    }

    fn read(&self) -> Result<SavedBoosts> {
        if !is_present(&self.directory, fs::FileType::is_dir)? {
            return Ok(SavedBoosts::default());
        }
        let current = self.directory.join(CURRENT);
        if !is_present(&current, fs::FileType::is_file)? {
            return Ok(SavedBoosts::default());
        }
        let text = fs::read_to_string(current).context("Read Boosts snapshot")?;
        let decoded = snapshot::decode(&text).context("Read Boosts snapshot")?;
        ensure!(
            decoded.repairs == snapshot::SnapshotRepairs::default(),
            "Boosts snapshot contains damaged entries"
        );
        let mut document: serde_json::Value = serde_json::from_str(&text)?;
        let disabled: BTreeSet<String> = document
            .as_object_mut()
            .and_then(|fields| fields.remove("disabled"))
            .map(serde_json::from_value)
            .transpose()
            .context("Read disabled Boosts")?
            .unwrap_or_default();
        ensure!(
            disabled
                .iter()
                .all(|name| decoded.boosts.get(name).is_some()),
            "Boosts snapshot references missing disabled entries"
        );
        Ok(SavedBoosts {
            all: decoded.boosts,
            disabled,
        })
    }

    fn write(&self, saved: &SavedBoosts) -> Result<()> {
        if !is_present(&self.directory, fs::FileType::is_dir)? {
            fs::create_dir(&self.directory)?;
        }
        let mut document: serde_json::Value = serde_json::from_str(&snapshot::encode(&saved.all))?;
        if !saved.disabled.is_empty() {
            document["disabled"] = serde_json::to_value(&saved.disabled)?;
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer_pretty(&mut file, &document)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join(CURRENT))?;
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn is_present(path: &Path, expected: fn(&fs::FileType) -> bool) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            ensure!(
                expected(&metadata.file_type()),
                "Unexpected file type in Boosts storage"
            );
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("Inspect Boosts storage"),
    }
}

pub(crate) fn compile_script(name: &BoostName, source: &str) -> CompiledBoost {
    compile::compile_with(name, source, |name, source| {
        transform(name, source).map_err(|error| format!("{error:#}"))
    })
}

fn transform(name: &str, source: &str) -> Result<String> {
    // Transform stdin without bundling or resolving anything from the filesystem.
    let mut child = Command::new("esbuild")
        .args([
            "--loader=ts",
            "--format=cjs",
            "--color=false",
            "--log-level=error",
        ])
        .arg(format!("--sourcefile={name}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Start esbuild (script Boosts require esbuild on PATH)")?;
    let input = child.stdin.take().expect("piped esbuild input");
    let output = std::thread::scope(|scope| {
        let write = scope.spawn(move || {
            let mut input = input;
            input.write_all(source.as_bytes())
        });
        let output = child.wait_with_output()?;
        if output.status.success() {
            write.join().expect("esbuild input writer")?;
        }
        Ok::<_, io::Error>(output)
    })?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_boost_edits_are_installed_in_the_engine() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        let name =
            controller.edit_boosts(|boosts| boosts.create("example.test", BoostKind::Styles))?;
        let css = "body { color: red; }";
        controller.edit_boosts(|boosts| boosts.save(&name, css, None))?;
        assert_eq!(
            controller.engine.boosts().get(name.as_str()).unwrap().code,
            css
        );
        drop(controller);

        let mut restarted = Controller::open_test_profile(profile.path())?;
        assert_eq!(
            restarted.engine.boosts().get(name.as_str()).unwrap().code,
            css
        );
        restarted.edit_boosts(|boosts| boosts.set_enabled(&name, false))?;
        assert!(restarted.engine.boosts().get(name.as_str()).is_none());
        restarted.edit_boosts(|boosts| boosts.set_enabled(&name, true))?;
        assert_eq!(
            restarted.engine.boosts().get(name.as_str()).unwrap().code,
            css
        );
        restarted.edit_boosts(|boosts| boosts.delete(&name))?;
        assert!(restarted.engine.boosts().get(name.as_str()).is_none());
        Ok(())
    }

    #[test]
    #[ignore = "requires the esbuild executable on PATH"]
    fn script_compiler_transforms_typescript_and_reports_syntax_errors() {
        let name = BoostName::for_host("example.com", BoostKind::Script).unwrap();
        let compiled = compile_script(&name, "export const size: number = 12;");
        assert_eq!(compiled.error, None);
        let javascript = compiled.code.unwrap();
        assert!(javascript.contains("size = 12"));
        assert!(!javascript.contains(": number"));
        assert!(javascript.contains("sourceURL=flecha-boost/example.com.ts"));

        let invalid = compile_script(&name, "const size: number = ;");
        assert!(invalid.code.is_none());
        assert!(invalid.error.unwrap().contains("example.com.ts:1"));
    }

    #[test]
    fn profiles_keep_source_and_enabled_state_across_restarts() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let directory = profile.path().join("boosts");
        let mut boosts = ProfileBoosts::open(directory.clone());
        let css = boosts.create("WWW.Example.com", BoostKind::Styles)?;
        let script = boosts.create("example.com", BoostKind::Script)?;
        boosts.save(&css, "body {\n  color: red;\n}", None)?;
        let build =
            compile::compile_with(
                &script,
                "let n: number = 1;",
                |_, _| Ok("let n = 1;".into()),
            );
        boosts.save(&script, "let n: number = 1;", Some(build))?;
        boosts.set_enabled(&css, false)?;
        assert_eq!(boosts.revision(), 5);
        assert!(boosts.enabled().get(css.as_str()).is_none());
        assert!(boosts.enabled().get(script.as_str()).is_some());
        let mut restarted = ProfileBoosts::open(directory.clone());
        assert_eq!(
            restarted.all().get(css.as_str()).unwrap().code,
            "body {\n  color: red;\n}"
        );
        assert!(restarted.enabled().get(css.as_str()).is_none());
        restarted.set_enabled(&css, true)?;
        assert!(restarted.enabled().get(css.as_str()).is_some());
        restarted.delete(&css)?;
        assert!(
            ProfileBoosts::open(directory)
                .all()
                .get(css.as_str())
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn unreadable_and_damaged_snapshots_cannot_be_overwritten() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(CURRENT);
        fs::create_dir(&path)?;
        let mut boosts = ProfileBoosts::open(directory.path().into());
        assert!(boosts.load_error().is_some());
        assert!(boosts.create("example.com", BoostKind::Styles).is_err());
        assert!(path.is_dir());

        let directory = tempfile::tempdir()?;
        let path = directory.path().join(CURRENT);
        let damaged = br#"{"version":1,"boosts":[{"name":"bad"}]}"#;
        let mut previously_loaded = ProfileBoosts::open(directory.path().into());
        fs::write(&path, damaged)?;
        assert!(
            previously_loaded
                .create("example.com", BoostKind::Styles)
                .is_err()
        );
        let mut reopened = ProfileBoosts::open(directory.path().into());
        assert!(reopened.load_error().is_some());
        assert!(reopened.create("example.com", BoostKind::Styles).is_err());
        assert_eq!(fs::read(path)?, damaged);
        Ok(())
    }
}
