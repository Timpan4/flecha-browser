//! Chromium's download file-type policy: `data/download_file_types.asciipb`, copied unchanged
//! from Chromium 158.0.8127.18 (`components/safe_browsing/content/resources/`, version_id 69).
//!
//! The parser keeps one setting per file type for one platform, as
//! `gen_file_type_proto.py`'s `PrunePlatformSettings` does at Chromium build time: the exact
//! platform, else the platform-less entry, else `default_file_type`'s setting.

use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

use crate::Platform;

const CHROMIUM_POLICY: &str = include_str!("../data/download_file_types.asciipb");

/// `DownloadFileType::DangerLevel`. A setting without `danger_level` gets proto2's first
/// enum value, `NOT_DANGEROUS`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DangerLevel {
    #[default]
    NotDangerous,
    AllowOnUserGesture,
    Dangerous,
}

#[derive(Debug)]
pub struct FileTypePolicies {
    version_id: u32,
    by_extension: HashMap<String, DangerLevel>,
    default: DangerLevel,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PolicyParseError(String);

impl fmt::Display for PolicyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "download file-type policy: {}", self.0)
    }
}

impl std::error::Error for PolicyParseError {}

impl FileTypePolicies {
    /// The vendored Chromium policy for `platform`.
    pub fn chromium(platform: Platform) -> &'static Self {
        static LINUX: LazyLock<FileTypePolicies> = LazyLock::new(|| vendored(Platform::Linux));
        static MAC: LazyLock<FileTypePolicies> = LazyLock::new(|| vendored(Platform::Mac));
        static WINDOWS: LazyLock<FileTypePolicies> = LazyLock::new(|| vendored(Platform::Windows));
        match platform {
            Platform::Linux => &LINUX,
            Platform::Mac => &MAC,
            Platform::Windows => &WINDOWS,
        }
    }

    /// Reads a `DownloadFileTypeConfig` text proto. Unknown fields are errors, so a format
    /// change can't silently drop a rule.
    pub fn parse(text: &str, platform: Platform) -> Result<Self, PolicyParseError> {
        let mut tokens = Tokens::new(text);
        let config = read_message(&mut tokens, true)?;
        let mut version_id = None;
        let mut default = None;
        let mut file_types = Vec::new();
        for (name, value) in config {
            match (name.as_str(), value) {
                ("version_id", Value::Scalar(v)) => {
                    version_id = Some(v.parse().map_err(|_| error("bad version_id"))?)
                }
                (
                    "sampled_ping_probability" | "max_archived_binaries_to_report",
                    Value::Scalar(_),
                ) => {}
                ("default_file_type", Value::Message(fields)) => default = Some(fields),
                ("file_types", Value::Message(fields)) => file_types.push(fields),
                (name, _) => return Err(error(&format!("unexpected config field {name}"))),
            }
        }
        let default = default.ok_or_else(|| error("no default_file_type"))?;
        let default = FileType::read(default)?
            .pick(platform)
            .ok_or_else(|| error("default_file_type has no setting for this platform"))?;
        let mut by_extension = HashMap::new();
        for fields in file_types {
            let file_type = FileType::read(fields)?;
            let extension = file_type
                .extension
                .clone()
                .ok_or_else(|| error("file type without an extension"))?;
            let level = file_type.pick(platform).unwrap_or(default);
            // FileTypePolicies::SwapConfigLocked: if there are duplicates, the first one wins.
            by_extension.entry(extension).or_insert(level);
        }
        Ok(Self {
            version_id: version_id.ok_or_else(|| error("no version_id"))?,
            by_extension,
            default,
        })
    }

    pub fn version_id(&self) -> u32 {
        self.version_id
    }

    pub fn extension_count(&self) -> usize {
        self.by_extension.len()
    }

    /// `FileTypePolicies::GetFileDangerLevel` for a file name, without enterprise overrides.
    pub fn danger_level(&self, file_name: &str) -> DangerLevel {
        self.by_extension
            .get(&canonical_extension(file_name))
            .copied()
            .unwrap_or(self.default)
    }
}

fn vendored(platform: Platform) -> FileTypePolicies {
    FileTypePolicies::parse(CHROMIUM_POLICY, platform)
        .expect("the vendored Chromium policy parses (tested)")
}

/// `FileTypePolicies::CanonicalizedExtension`: the final extension after trailing dots and
/// spaces are trimmed, lowercased. A non-ASCII extension can't match the ASCII list, so it is
/// empty.
pub fn canonical_extension(file_name: &str) -> String {
    let base = file_name.rsplit('/').next().unwrap_or(file_name);
    let trimmed = base.trim_end_matches(['.', ' ']);
    if trimmed == "." || trimmed == ".." {
        return String::new();
    }
    match trimmed.rfind('.') {
        Some(dot) if trimmed[dot + 1..].is_ascii() => trimmed[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

struct FileType {
    extension: Option<String>,
    settings: Vec<(u8, DangerLevel)>,
}

impl FileType {
    fn read(fields: Vec<(String, Value)>) -> Result<Self, PolicyParseError> {
        let mut extension = None;
        let mut settings = Vec::new();
        for (name, value) in fields {
            match (name.as_str(), value) {
                ("extension", Value::Scalar(v)) => extension = Some(v),
                (
                    "uma_value" | "is_archive" | "ping_setting" | "inspection_type",
                    Value::Scalar(_),
                ) => {}
                ("platform_settings", Value::Message(fields)) => {
                    let mut platform = 0;
                    let mut level = DangerLevel::NotDangerous;
                    for (name, value) in fields {
                        let Value::Scalar(value) = value else {
                            return Err(error(&format!("{name} is not a scalar")));
                        };
                        match name.as_str() {
                            "platform" => platform = platform_number(&value)?,
                            "danger_level" => level = danger_level(&value)?,
                            "auto_open_hint" | "max_file_size_to_analyze" | "file_weight" => {}
                            name => return Err(error(&format!("unexpected setting {name}"))),
                        }
                    }
                    settings.push((platform, level));
                }
                (name, _) => return Err(error(&format!("unexpected file type field {name}"))),
            }
        }
        Ok(Self {
            extension,
            settings,
        })
    }

    /// PrunePlatformSettings: an exact platform match wins over PLATFORM_TYPE_ANY.
    fn pick(&self, platform: Platform) -> Option<DangerLevel> {
        let wanted = match platform {
            Platform::Linux => 3,
            Platform::Mac => 4,
            Platform::Windows => 5,
        };
        let exact = self.settings.iter().find(|(p, _)| *p == wanted);
        let any = self.settings.iter().find(|(p, _)| *p == 0);
        exact.or(any).map(|(_, level)| *level)
    }
}

fn platform_number(value: &str) -> Result<u8, PolicyParseError> {
    Ok(match value {
        "PLATFORM_TYPE_ANY" => 0,
        "PLATFORM_TYPE_ANDROID" => 1,
        "PLATFORM_TYPE_CHROME_OS" => 2,
        "PLATFORM_TYPE_LINUX" => 3,
        "PLATFORM_TYPE_MAC" => 4,
        "PLATFORM_TYPE_WINDOWS" => 5,
        other => return Err(error(&format!("unknown platform {other}"))),
    })
}

fn danger_level(value: &str) -> Result<DangerLevel, PolicyParseError> {
    Ok(match value {
        "NOT_DANGEROUS" => DangerLevel::NotDangerous,
        "ALLOW_ON_USER_GESTURE" => DangerLevel::AllowOnUserGesture,
        "DANGEROUS" => DangerLevel::Dangerous,
        other => return Err(error(&format!("unknown danger level {other}"))),
    })
}

fn error(message: &str) -> PolicyParseError {
    PolicyParseError(message.into())
}

enum Value {
    Scalar(String),
    Message(Vec<(String, Value)>),
}

fn read_message(
    tokens: &mut Tokens<'_>,
    top_level: bool,
) -> Result<Vec<(String, Value)>, PolicyParseError> {
    let mut fields = Vec::new();
    loop {
        let Some(token) = tokens.next()? else {
            return if top_level {
                Ok(fields)
            } else {
                Err(error("unclosed message"))
            };
        };
        let name = match token {
            Token::Close if !top_level => return Ok(fields),
            Token::Word(name) => name,
            _ => return Err(error("expected a field name")),
        };
        let value = match tokens.next()? {
            Some(Token::Colon) => match tokens.next()? {
                Some(Token::Word(word)) => Value::Scalar(word),
                Some(Token::Quoted(text)) => Value::Scalar(text),
                _ => return Err(error(&format!("{name} has no value"))),
            },
            Some(Token::Open) => Value::Message(read_message(tokens, false)?),
            _ => return Err(error(&format!("{name} is neither a value nor a message"))),
        };
        fields.push((name, value));
    }
}

enum Token {
    Word(String),
    Quoted(String),
    Colon,
    Open,
    Close,
}

struct Tokens<'a> {
    rest: &'a str,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        Self { rest: text }
    }

    fn next(&mut self) -> Result<Option<Token>, PolicyParseError> {
        loop {
            self.rest = self.rest.trim_start();
            if self.rest.starts_with('#') {
                self.rest = self.rest.split_once('\n').map_or("", |(_, rest)| rest);
            } else {
                break;
            }
        }
        let Some(first) = self.rest.chars().next() else {
            return Ok(None);
        };
        let token = match first {
            ':' => Token::Colon,
            '{' => Token::Open,
            '}' => Token::Close,
            '"' => {
                let body = &self.rest[1..];
                let end = body.find('"').ok_or_else(|| error("unterminated string"))?;
                if body[..end].contains('\\') {
                    return Err(error("escaped strings are not supported"));
                }
                self.rest = &body[end + 1..];
                return Ok(Some(Token::Quoted(body[..end].into())));
            }
            c if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' => {
                let end = self
                    .rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
                    .unwrap_or(self.rest.len());
                let word = &self.rest[..end];
                self.rest = &self.rest[end..];
                return Ok(Some(Token::Word(word.into())));
            }
            other => return Err(error(&format!("unexpected character {other:?}"))),
        };
        self.rest = &self.rest[1..];
        Ok(Some(token))
    }
}
