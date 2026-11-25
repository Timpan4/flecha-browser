use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail, ensure};
use cef::{rc::Rc as _, *};
use serde_json::{Value, json};
use url::Url;

pub const STORE_UPDATE_URL: &str = "https://clients2.google.com/service/update2/crx";
pub const MANAGED_POLICY_SUPPORTED: bool = cfg!(target_os = "linux");
const POLICY_DIR: &str = "extension-policy";
const POLICY_FILE: &str = "managed/flecha.json";
const SETTINGS_FILE: &str = "extensions.json";
pub(crate) const PREF: &str = "extensions.settings";

wrap_preference_observer! {
    struct ExtensionObserver { dirty: Rc<Cell<bool>> }
    impl PreferenceObserver {
        fn on_preference_changed(&self, _name: Option<&CefString>) { self.dirty.set(true); }
    }
}

pub(crate) struct Watch {
    registration: Option<Registration>,
    dirty: Rc<Cell<bool>>,
    pub installed: Vec<Extension>,
}

impl Watch {
    pub fn new(context: &RequestContext) -> Self {
        let dirty = Rc::new(Cell::new(true));
        let mut observer = ExtensionObserver::new(dirty.clone());
        let registration = PreferenceManager::from(context)
            .add_preference_observer(Some(&PREF.into()), Some(&mut observer));
        Self {
            registration,
            dirty,
            installed: Vec::new(),
        }
    }

    pub fn refresh(
        &mut self,
        context: &RequestContext,
        profile: &Path,
        unpacked: &BTreeSet<PathBuf>,
    ) -> bool {
        // Without an observer registration nothing marks the watch dirty, so every poll reads.
        if !self.dirty.replace(self.registration.is_none()) {
            return false;
        }
        let preferences = PreferenceManager::from(context)
            .preference(Some(&PREF.into()))
            .and_then(|mut value| {
                let json = CefString::from(&cef::write_json(
                    Some(&mut value),
                    JsonWriterOptions::default(),
                ));
                serde_json::from_str(&json.to_string()).ok()
            })
            .or_else(|| disk_preferences(profile).ok());
        let Some(preferences) = preferences else {
            self.dirty.set(true);
            return false;
        };
        let installed = inventory(preferences, profile, unpacked);
        if installed == self.installed {
            return false;
        }
        self.installed = installed;
        true
    }
}

pub fn store_id(input: &str) -> Result<String> {
    let input = input.trim();
    if valid_id(input) {
        return Ok(input.to_owned());
    }
    let url = Url::parse(input).context("Enter a Chrome Web Store link or extension ID")?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "Enter a Chrome Web Store link or extension ID"
    );
    let path = url.path().trim_end_matches('/');
    let detail = match url.host_str() {
        Some("chromewebstore.google.com") => path.strip_prefix("/detail/"),
        Some("chrome.google.com") => path.strip_prefix("/webstore/detail/"),
        _ => None,
    }
    .context("Enter a Chrome Web Store detail link")?;
    let (slug, id) = detail.split_once('/').unwrap_or(("", detail));
    ensure!(
        !slug.contains('/') && valid_id(id),
        "Invalid Chrome Web Store link"
    );
    Ok(id.to_owned())
}

fn valid_id(id: &str) -> bool {
    // Chromium extension IDs encode 128 bits with the letters a through p.
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

#[derive(Clone)]
pub struct ExtensionSettings {
    profile: PathBuf,
}

impl ExtensionSettings {
    pub fn new(profile: &Path) -> Self {
        Self {
            profile: profile.to_owned(),
        }
    }

    pub fn policy_directory(&self) -> PathBuf {
        self.profile.join(POLICY_DIR)
    }

    pub fn managed_ids(&self) -> Result<BTreeSet<String>> {
        Ok(self.policy()?.into_keys().collect())
    }

    pub fn add_store(&self, id: &str) -> Result<()> {
        ensure!(
            MANAGED_POLICY_SUPPORTED,
            "Managed Store installs are available on Linux"
        );
        ensure!(valid_id(id), "Invalid extension ID");
        let mut policy = self.policy()?;
        policy.insert(id.to_owned(), STORE_UPDATE_URL.to_owned());
        self.write_policy(policy)
    }

    pub fn remove_managed(&self, id: &str) -> Result<()> {
        let mut policy = self.policy()?;
        ensure!(
            policy.remove(id).is_some(),
            "Extension is no longer managed by Flecha"
        );
        self.write_policy(policy)
    }

    fn policy(&self) -> Result<BTreeMap<String, String>> {
        let path = self.policy_directory().join(POLICY_FILE);
        let value = read_json(&path)?;
        let Some(list) = value.get("ExtensionInstallForcelist") else {
            return Ok(BTreeMap::new());
        };
        let policy: Result<BTreeMap<String, String>> = (|| {
            list.as_array()
                .context("Invalid ExtensionInstallForcelist")?
                .iter()
                .map(|entry| {
                    let entry = entry.as_str().context("Invalid managed extension entry")?;
                    // Chromium's policy reads a bare ID as a Chrome Web Store install.
                    let (id, url) = entry.split_once(';').unwrap_or((entry, STORE_UPDATE_URL));
                    ensure!(valid_id(id), "Invalid managed extension ID");
                    Ok((id.to_owned(), url.to_owned()))
                })
                .collect()
        })();
        policy.with_context(|| format!("Read {}", path.display()))
    }

    /// Replaces the force-install list and keeps any other policy in the file.
    fn write_policy(&self, policy: BTreeMap<String, String>) -> Result<()> {
        let path = self.policy_directory().join(POLICY_FILE);
        let list: Vec<_> = policy
            .into_iter()
            .map(|(id, url)| format!("{id};{url}"))
            .collect();
        let mut value = read_json(&path)?;
        value["ExtensionInstallForcelist"] = json!(list);
        write_json(&path, &value)
    }

    pub fn unpacked(&self) -> Result<BTreeSet<PathBuf>> {
        let settings = read_json(&self.profile.join(SETTINGS_FILE))?;
        settings.get("unpacked").cloned().map_or_else(
            || Ok(BTreeSet::new()),
            |paths| serde_json::from_value(paths).context("Invalid unpacked extension folders"),
        )
    }

    /// The folders a launch passes to `--load-extension`. One unusable path skips them all.
    pub fn startup_folders(&self) -> Result<BTreeSet<PathBuf>> {
        let folders = self.unpacked()?;
        for folder in &folders {
            unpacked_argument(folder)?;
        }
        Ok(folders)
    }

    pub fn add_unpacked(&self, directory: &Path) -> Result<()> {
        let directory = directory.canonicalize().context("Open extension folder")?;
        unpacked_argument(&directory)?;
        let manifest = load_manifest(&directory)?;
        ensure!(
            manifest.get("manifest_version").and_then(Value::as_u64) == Some(3),
            "Select a Manifest V3 extension folder"
        );
        let mut paths = self.unpacked()?;
        paths.insert(directory);
        self.write_unpacked(paths)
    }

    pub fn remove_unpacked(&self, directory: &Path) -> Result<()> {
        let mut paths = self.unpacked()?;
        ensure!(
            paths.remove(directory),
            "Extension folder is no longer configured"
        );
        self.write_unpacked(paths)
    }

    fn write_unpacked(&self, paths: BTreeSet<PathBuf>) -> Result<()> {
        let mut settings = read_json(&self.profile.join(SETTINGS_FILE))?;
        settings["unpacked"] = json!(paths);
        write_json(&self.profile.join(SETTINGS_FILE), &settings)
    }

    pub fn pinned(&self) -> Result<BTreeSet<String>> {
        let settings = read_json(&self.profile.join(SETTINGS_FILE))?;
        settings.get("pinned").cloned().map_or_else(
            || Ok(BTreeSet::new()),
            |ids| serde_json::from_value(ids).context("Invalid pinned extensions"),
        )
    }

    pub fn toggle_pin(&self, id: &str) -> Result<()> {
        ensure!(valid_id(id), "Invalid extension ID");
        let mut pins = self.pinned()?;
        if !pins.remove(id) {
            pins.insert(id.to_owned());
        }
        let mut settings = read_json(&self.profile.join(SETTINGS_FILE))?;
        settings["pinned"] = json!(pins);
        write_json(&self.profile.join(SETTINGS_FILE), &settings)
    }
}

pub(crate) fn unpacked_argument(directory: &Path) -> Result<&str> {
    let text = directory
        .to_str()
        .context("Extension folders must have UTF-8 paths")?;
    ensure!(
        directory.is_absolute() && !text.contains(','),
        "Use an absolute folder path without commas"
    );
    Ok(text)
}

fn read_json(path: &Path) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("Read {}", path.display()))?;
            ensure!(
                value.is_object(),
                "Expected an object in {}",
                path.display()
            );
            Ok(value)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error).with_context(|| format!("Read {}", path.display())),
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().context("Missing settings directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

pub fn load_manifest(directory: &Path) -> Result<Value> {
    let file = fs::File::open(directory.join("manifest.json")).context("Open manifest.json")?;
    serde_json::from_reader(file).context("Read manifest.json")
}

/// Review the manifest in a Store CRX before putting its ID in policy. Chromium verifies
/// the CRX signature itself when the policy install runs.
pub fn crx_manifest(path: &Path) -> Result<Value> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut package = fs::File::open(path)?;
    let mut header = [0; 12];
    package.read_exact(&mut header)?;
    ensure!(
        header[..8] == *b"Cr24\x03\0\0\0",
        "Store response is not a CRX3 package"
    );
    let [a, b, c, d] = header[8..] else {
        unreachable!()
    };
    package.seek(SeekFrom::Current(i64::from(u32::from_le_bytes([
        a, b, c, d,
    ]))))?;
    let mut zip = tempfile::NamedTempFile::new()?;
    std::io::copy(&mut package, &mut zip)?;
    let path = crate::runtime::path_string(zip.path())?;
    let mut stream = stream_reader_create_for_file(Some(&path)).context("Open Store package")?;
    let reader = zip_reader_create(Some(&mut stream)).context("Read Store package ZIP")?;
    let mut manifest = archive_json(&reader, "manifest.json")?;
    if let Some(name) = archive_name(&reader, &manifest) {
        manifest["name"] = json!(name);
    }
    Ok(manifest)
}

fn archive_name(reader: &ZipReader, manifest: &Value) -> Option<String> {
    let name = manifest.get("name")?.as_str()?;
    let key = name.strip_prefix("__MSG_")?.strip_suffix("__")?;
    let locale = manifest.get("default_locale")?.as_str()?;
    if !locale
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    let messages = archive_json(reader, &format!("_locales/{locale}/messages.json")).ok()?;
    let localized = messages
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))?
        .1;
    Some(localized.get("message")?.as_str()?.to_owned())
}

fn archive_json(reader: &ZipReader, path: &str) -> Result<Value> {
    use std::io::{BufReader, Read};
    ensure!(
        reader.move_to_file(Some(&path.into()), 1) == 1 && reader.open_file(None) == 1,
        "Store package has no readable {path}"
    );
    struct Entry<'a>(&'a ZipReader);
    impl Read for Entry<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let mut buffer = vec![0; bytes.len()];
            let size = usize::try_from(self.0.read_file(Some(&mut buffer)))
                .map_err(|_| std::io::Error::other("Read Store package manifest"))?;
            let data = buffer
                .get(..size)
                .context("Invalid ZIP read size")
                .map_err(std::io::Error::other)?;
            bytes[..size].copy_from_slice(data);
            Ok(size)
        }
    }
    impl Drop for Entry<'_> {
        fn drop(&mut self) {
            self.0.close_file();
        }
    }
    serde_json::from_reader(BufReader::new(Entry(reader)))
        .with_context(|| format!("Read Store package {path}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    pub id: String,
    pub name: String,
    pub version: String,
    pub icon: Option<PathBuf>,
    pub has_action: bool,
    pub popup: Option<String>,
    pub unpacked: Option<PathBuf>,
    pub enabled: bool,
}

impl Extension {
    fn load(id: &str, pref: &Value, profile: &Path, unpacked: &BTreeSet<PathBuf>) -> Option<Self> {
        if !valid_id(id) {
            return None;
        }
        let path = Path::new(pref.get("path")?.as_str()?);
        let (root, unpacked) = if path.is_absolute() && unpacked.contains(path) {
            (path.to_owned(), Some(path.to_owned()))
        } else {
            let mut components = path.components();
            if components.next() != Some(Component::Normal(id.as_ref()))
                || !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
            {
                return None;
            }
            (profile.join("Extensions").join(path), None)
        };
        let manifest = load_manifest(&root).ok()?;
        let action = manifest
            .get("action")
            .or_else(|| manifest.get("browser_action"));
        let icon = action
            .and_then(|action| icon_path(action.get("default_icon")?))
            .or_else(|| icon_path(manifest.get("icons")?))
            .and_then(|path| extension_file(&root, path));
        let popup = action
            .and_then(|action| action.get("default_popup")?.as_str())
            .and_then(|path| popup_url(id, path));
        let name = pref
            .pointer("/manifest/name")
            .and_then(Value::as_str)
            .or_else(|| manifest.get("name").and_then(Value::as_str))
            .unwrap_or(id);
        Some(Self {
            id: id.to_owned(),
            name: name.to_owned(),
            version: manifest.get("version")?.as_str()?.to_owned(),
            icon,
            has_action: action.is_some(),
            popup,
            unpacked,
            enabled: pref
                .get("disable_reasons")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty),
        })
    }
}

fn icon_path(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| {
        value
            .as_object()?
            .iter()
            .filter_map(|(size, path)| Some((size.parse::<u32>().ok()?, path.as_str()?)))
            .min_by_key(|(size, _)| (size.abs_diff(32), *size))
            .map(|(_, path)| path)
    })
}

fn extension_file(root: &Path, path: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let file = root
        .join(path.trim_start_matches('/'))
        .canonicalize()
        .ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn popup_url(id: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\\') {
        return None;
    }
    let base = Url::parse(&format!("chrome-extension://{id}/")).ok()?;
    let url = base.join(path).ok()?;
    (url.scheme() == "chrome-extension" && url.host_str() == Some(id) && url.path() != "/")
        .then(|| url.into())
}

pub(crate) fn inventory(
    preferences: Value,
    profile: &Path,
    unpacked: &BTreeSet<PathBuf>,
) -> Vec<Extension> {
    preferences
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, pref)| Extension::load(id, pref, profile, unpacked))
        .collect()
}

pub(crate) fn disk_preferences(profile: &Path) -> Result<Value> {
    let mut entries = BTreeMap::new();
    for file in ["Preferences", "Secure Preferences"] {
        let prefs = read_json(&profile.join(file))?;
        if let Some(extensions) = prefs
            .pointer("/extensions/settings")
            .and_then(Value::as_object)
        {
            entries.extend(
                extensions
                    .iter()
                    .map(|(id, entry)| (id.clone(), entry.clone())),
            );
        }
    }
    Ok(json!(entries))
}

pub fn requested_permissions(manifest: &Value) -> Result<Vec<String>> {
    ensure!(manifest.is_object(), "Invalid extension manifest");
    let mut permissions = BTreeSet::new();
    for field in [
        "permissions",
        "host_permissions",
        "optional_permissions",
        "optional_host_permissions",
    ] {
        if let Some(values) = manifest.get(field) {
            let values = values
                .as_array()
                .context("Invalid permissions in extension manifest")?;
            for value in values {
                let Some(value) = value.as_str() else {
                    bail!("Invalid permission in extension manifest")
                };
                let label = if field.starts_with("optional_") {
                    format!("{value} (optional)")
                } else {
                    value.to_owned()
                };
                permissions.insert(label);
            }
        }
    }
    if let Some(scripts) = manifest.get("content_scripts").and_then(Value::as_array) {
        for host in scripts
            .iter()
            .filter_map(|script| script.get("matches")?.as_array())
            .flatten()
        {
            if let Some(host) = host.as_str() {
                permissions.insert(format!("Run scripts on {host}"));
            }
        }
    }
    if manifest.get("devtools_page").is_some() {
        permissions.insert("Run code in pages you inspect with DevTools".to_owned());
    }
    Ok(permissions.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_lists_every_capability_the_manifest_declares() {
        let manifest = json!({
            "permissions": ["storage"],
            "optional_host_permissions": ["https://example.com/*"],
            "content_scripts": [{ "matches": ["https://*/*"] }],
            "devtools_page": "devtools.html",
        });
        assert_eq!(
            requested_permissions(&manifest).unwrap(),
            [
                "Run code in pages you inspect with DevTools",
                "Run scripts on https://*/*",
                "https://example.com/* (optional)",
                "storage",
            ]
        );
        assert!(requested_permissions(&json!({ "permissions": "storage" })).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn managed_edits_keep_bare_ids_and_other_policies() {
        let profile = tempfile::tempdir().unwrap();
        let settings = ExtensionSettings::new(profile.path());
        let path = settings.policy_directory().join(POLICY_FILE);
        let bare = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        let added = "eimadpbcbfnmbkopoojfekhnkhdbieeh";
        write_json(
            &path,
            &json!({ "ExtensionInstallForcelist": [bare], "BlockExternalExtensions": true }),
        )
        .unwrap();
        settings.add_store(added).unwrap();
        assert_eq!(
            read_json(&path).unwrap(),
            json!({
                "ExtensionInstallForcelist": [
                    format!("{bare};{STORE_UPDATE_URL}"),
                    format!("{added};{STORE_UPDATE_URL}"),
                ],
                "BlockExternalExtensions": true,
            })
        );
        write_json(&path, &json!({ "ExtensionInstallForcelist": 5 })).unwrap();
        let error = format!("{:#}", settings.managed_ids().unwrap_err());
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn store_ids_come_only_from_ids_and_store_detail_links() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        for input in [
            id.to_owned(),
            format!("https://chromewebstore.google.com/detail/ublock/{id}"),
            format!("https://chrome.google.com/webstore/detail/ublock/{id}/"),
        ] {
            assert_eq!(store_id(&input).unwrap(), id, "{input}");
        }
        for input in [
            format!("http://chromewebstore.google.com/detail/ublock/{id}"),
            format!("https://user@chromewebstore.google.com/detail/ublock/{id}"),
            format!("https://example.com/detail/ublock/{id}"),
            format!("https://chromewebstore.google.com/detail/a/b/{id}"),
            "cjpalhdlnbpafiamejdnhcphjbkeiagz".to_owned(),
        ] {
            assert!(store_id(&input).is_err(), "{input}");
        }
    }
}
