use std::borrow::Cow;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Component, Path};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use serde_json::Value;

use crate::ReadError;

mod json;
pub(crate) use json::{JsonRead, arc_sidebar_json, json};

// Node's fs.readFile contract for the shipping profile reader.
pub(crate) const MAX_TEXT_BYTES: u64 = i32::MAX as u64;

pub(crate) fn open_root(path: &Path) -> Result<Dir, ReadError> {
    let ambient = |path: &Path| Dir::open_ambient_dir(path, ambient_authority()).map_err(io_error);
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        open_dir(&ambient(parent)?, Path::new(name))
    } else {
        ambient(path)
    }
}

pub(crate) fn open_dir(root: &Dir, path: &Path) -> Result<Dir, ReadError> {
    let mut dir = root.try_clone().map_err(io_error)?;
    for part in path.components() {
        let Component::Normal(part) = part else {
            return Err(ReadError::UnsafeFile);
        };
        if dir.symlink_metadata(part).map_err(io_error)?.is_symlink() {
            return Err(ReadError::UnsafeFile);
        }
        dir = dir.open_dir_nofollow(part).map_err(io_error)?;
    }
    Ok(dir)
}

pub(crate) fn dirs(dir: &Dir) -> Result<Vec<OsString>, ReadError> {
    let mut names = Vec::new();
    for entry in dir.entries().map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if entry.file_type().map_err(io_error)?.is_dir() {
            names.push(entry.file_name());
        }
    }
    Ok(names)
}

pub(crate) fn file_exists(dir: &Dir, name: &str) -> Result<bool, ReadError> {
    match dir.symlink_metadata(name) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(ReadError::UnsafeFile),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io_error(err)),
    }
}

pub(crate) fn file(dir: &Dir, name: &str) -> Result<File, ReadError> {
    open_file(dir, Path::new(name))
}

fn open_file(dir: &Dir, name: &Path) -> Result<File, ReadError> {
    if !dir.symlink_metadata(name).map_err(io_error)?.is_file() {
        return Err(ReadError::UnsafeFile);
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // A regular file can be replaced with a FIFO between stat and open.
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = dir.open_with(name, &options).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(ReadError::UnsafeFile);
    }
    Ok(file)
}

pub(crate) fn read_selected_text(path: &Path) -> Result<String, ReadError> {
    let name = path.file_name().ok_or(ReadError::UnsafeFile)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    read_text(open_file(&open_root(parent)?, Path::new(name))?)
}

pub(crate) fn read_text(file: File) -> Result<String, ReadError> {
    let data = read_bytes(file, MAX_TEXT_BYTES)?;
    Ok(String::from_utf8(data)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned()))
}

pub(crate) fn read_bytes(file: File, limit: u64) -> Result<Vec<u8>, ReadError> {
    if file.metadata().map_err(io_error)?.len() > limit {
        return Err(ReadError::TooLarge);
    }
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(io_error)?;
    if data.len() as u64 > limit {
        return Err(ReadError::TooLarge);
    }
    Ok(data)
}

fn escape_unit(bytes: &[u8]) -> Option<u16> {
    let [b'\\', b'u', digits @ ..] = bytes.get(..6)? else {
        return None;
    };
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

fn repair_surrogates(text: &str) -> Cow<'_, str> {
    let mut output = String::new();
    let mut start = 0;
    let mut at = 0;
    while let Some(next) = text[at..].find('\\') {
        at += next;
        if let Some(unit @ 0xd800..=0xdfff) = escape_unit(&text.as_bytes()[at..]) {
            if unit <= 0xdbff
                && matches!(
                    escape_unit(&text.as_bytes()[at + 6..]),
                    Some(0xdc00..=0xdfff)
                )
            {
                at += 12;
            } else {
                output.push_str(&text[start..at]);
                output.push_str(r"\ufffd");
                at += 6;
                start = at;
            }
        } else {
            // Skip an escaped backslash without skipping into a UTF-8 code point.
            at += 1;
            if let Some(ch) = text[at..].chars().next() {
                at += ch.len_utf8();
            }
        }
    }
    if start == 0 {
        Cow::Borrowed(text)
    } else {
        output.push_str(&text[start..]);
        Cow::Owned(output)
    }
}

pub(crate) fn io_error(error: io::Error) -> ReadError {
    match error.kind() {
        io::ErrorKind::NotFound => ReadError::Missing,
        io::ErrorKind::PermissionDenied => ReadError::PermissionDenied,
        _ => ReadError::Unreadable,
    }
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

pub(crate) fn js_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        Value::Number(value) => {
            let value = value.as_f64().expect("JSON numbers are finite");
            if value == 0.0 {
                "0".into()
            } else if value.abs() >= 1e21 {
                format!("{value:e}").replace('e', "e+")
            } else if value.abs() < 1e-6 {
                format!("{value:e}")
            } else {
                value.to_string()
            }
        }
        _ => value.to_string(),
    }
}

pub(crate) fn text_or<'a>(value: &'a Value, fallback: &'a str) -> &'a str {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

pub(crate) fn web_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
    })
}
