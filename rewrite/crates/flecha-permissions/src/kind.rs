/// A permission a page can request.
///
/// Chromium allows every one of these only in a secure context, so an origin that is not
/// potentially trustworthy is always blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PermissionKind {
    Geolocation,
    Camera,
    Microphone,
    Notifications,
    ClipboardRead,
}

impl PermissionKind {
    pub const ALL: [Self; 5] = [
        Self::Geolocation,
        Self::Camera,
        Self::Microphone,
        Self::Notifications,
        Self::ClipboardRead,
    ];

    /// The name saved in snapshots.
    pub fn name(self) -> &'static str {
        match self {
            Self::Geolocation => "geolocation",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::Notifications => "notifications",
            Self::ClipboardRead => "clipboard-read",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The kinds in one request, such as camera and microphone for one `getUserMedia` call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionSet(u8);

impl PermissionSet {
    pub const EMPTY: Self = Self(0);

    pub fn contains(self, kind: PermissionKind) -> bool {
        self.0 & kind.bit() != 0
    }

    pub fn insert(&mut self, kind: PermissionKind) {
        self.0 |= kind.bit();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = PermissionKind> {
        PermissionKind::ALL
            .into_iter()
            .filter(move |kind| self.contains(*kind))
    }
}

impl From<PermissionKind> for PermissionSet {
    fn from(kind: PermissionKind) -> Self {
        Self(kind.bit())
    }
}

impl From<SiteControl> for PermissionSet {
    fn from(control: SiteControl) -> Self {
        control.kinds()
    }
}

impl FromIterator<PermissionKind> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = PermissionKind>>(iter: I) -> Self {
        let mut set = Self::EMPTY;
        for kind in iter {
            set.insert(kind);
        }
        set
    }
}

/// A row in the URL bar's site panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SiteControl {
    Geolocation,
    /// Camera and microphone together.
    Media,
    Notifications,
    ClipboardRead,
}

impl SiteControl {
    pub const ALL: [Self; 4] = [
        Self::Geolocation,
        Self::Media,
        Self::Notifications,
        Self::ClipboardRead,
    ];

    /// The site panel's permission name in `chromium/shell/siteControls.ts`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Geolocation => "geolocation",
            Self::Media => "media",
            Self::Notifications => "notifications",
            Self::ClipboardRead => "clipboard-read",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|control| control.name() == name)
    }

    pub fn kinds(self) -> PermissionSet {
        match self {
            Self::Geolocation => PermissionKind::Geolocation.into(),
            Self::Media => [PermissionKind::Camera, PermissionKind::Microphone]
                .into_iter()
                .collect(),
            Self::Notifications => PermissionKind::Notifications.into(),
            Self::ClipboardRead => PermissionKind::ClipboardRead.into(),
        }
    }
}

/// A site-panel setting. `Ask` is the default and is never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
    Ask,
}

/// The answer a request gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
}

impl From<Verdict> for Decision {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Allow => Self::Allow,
            Verdict::Block => Self::Block,
        }
    }
}
