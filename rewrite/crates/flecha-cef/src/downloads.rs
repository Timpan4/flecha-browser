//! CEF's downloads, named, placed and held by flecha-downloads.
//!
//! `OnBeforeDownload` runs [`flecha_downloads::decide`]. A download that may save at once
//! continues to its decided path. One that must ask where to save, or that is held for a
//! warning, keeps CEF's `BeforeDownloadCallback` until the shell answers through
//! [`crate::Engine::accept_download`] or [`crate::Engine::cancel_download`]. Dropping that
//! callback unanswered cancels the download, which is how a refusal, a discard and a closed
//! browser end it.
//!
//! The path handed to CEF is the list's record of the download, so an intermediate name CEF
//! reports never replaces it (issue #114). When the person picks the path, the insecure-delivery
//! check runs again on the chosen file name.
//!
//! Answering or dropping a CEF callback can call straight back into this handler, so
//! [`Downloads`] never touches one: it returns [`Act`]s, which [`run`] carries out once the
//! `RefCell` borrow has ended. That covers releasing a callback too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use cef::{rc::Rc as _, *};
use chrono::NaiveDateTime;
use flecha_core::{EpochMillis, TabId};
use flecha_downloads::{
    AskReason, DangerSignals, DownloadId, DownloadList, DownloadRequest, DownloadSettings,
    InsecureSignals, InsecureVerdict, ListError, NameSources, NewDownload, Platform, Snapshot,
    SnapshotState, State, TargetDecision, Trigger, Verdict, Warning, decide, insecure_verdict,
};
use url::Url;

use crate::callbacks::Slot;
use crate::services::{self, Services};
use crate::{IncognitoSession, Partition};

/// A download in this engine session. The engine allocates it: CEF's download ids are unique
/// only within one request context, so a shared and an isolated tab can both start download 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DownloadHandle(pub u32);

/// CEF's name for a download: its request context's partition and its id there.
pub(crate) type CefDownload = (Option<crate::Partition>, u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Where should it go? Answer with [`crate::Engine::accept_download`] and the chosen absolute
    /// path, or [`crate::Engine::cancel_download`].
    AskPath {
        handle: DownloadHandle,
        url: String,
        suggested: PathBuf,
        reason: AskReason,
    },
    /// Waiting for the person to keep ([`crate::Engine::accept_download`] with no path) or
    /// discard ([`crate::Engine::cancel_download`]) it.
    Held {
        handle: DownloadHandle,
        id: DownloadId,
        warning: Warning,
    },
    /// CEF is saving it to `path`.
    Started {
        handle: DownloadHandle,
        id: DownloadId,
        path: PathBuf,
    },
    /// The list's row for `id` changed.
    Updated {
        handle: DownloadHandle,
        id: DownloadId,
    },
    /// Refused without asking: an insecure download Chromium blocks silently, or no download
    /// settings yet.
    Blocked { handle: DownloadHandle, url: String },
    /// Ended before it finished: discarded, cancelled while waiting, or its browser closed.
    Cancelled {
        handle: DownloadHandle,
        id: Option<DownloadId>,
    },
}

impl DownloadEvent {
    pub fn handle(&self) -> DownloadHandle {
        match self {
            Self::AskPath { handle, .. }
            | Self::Held { handle, .. }
            | Self::Started { handle, .. }
            | Self::Updated { handle, .. }
            | Self::Blocked { handle, .. }
            | Self::Cancelled { handle, .. } => *handle,
        }
    }

    fn id(&self) -> Option<DownloadId> {
        match self {
            Self::Held { id, .. } | Self::Started { id, .. } | Self::Updated { id, .. } => {
                Some(*id)
            }
            Self::Cancelled { id, .. } => *id,
            Self::AskPath { .. } | Self::Blocked { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadMessage {
    /// The tab that started it, while the engine still owns that tab's browser.
    pub tab: Option<TabId>,
    /// The incognito session it downloads in. Only that session's windows list it, as each
    /// Chromium profile keeps its own downloads.
    pub incognito: Option<IncognitoSession>,
    pub event: DownloadEvent,
}

/// The settings downloads follow. Without them every download is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadConfig {
    pub settings: DownloadSettings,
    /// Chromium's HTTPS-First Mode pref.
    pub https_first_mode: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// No download with this handle is waiting for an answer.
    NotWaiting(DownloadHandle),
    /// CEF hasn't reported this download, or it already ended.
    Unknown(DownloadHandle),
    /// A chosen path must be absolute, name a file and be UTF-8, which CEF requires. The prompt
    /// stays open.
    InvalidPath(PathBuf),
    /// A held download is kept at its decided path. Choosing another path is asked first.
    KeepsDecidedPath(DownloadHandle),
    /// Another download in progress is saving to this path. The prompt stays open.
    PathInUse(PathBuf),
    List(ListError),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWaiting(handle) => {
                write!(f, "download {} isn't waiting for an answer", handle.0)
            }
            Self::Unknown(handle) => write!(f, "no live download {}", handle.0),
            Self::InvalidPath(path) => {
                write!(f, "{} is not an absolute UTF-8 file path", path.display())
            }
            Self::PathInUse(path) => {
                write!(f, "another download is saving to {}", path.display())
            }
            Self::KeepsDecidedPath(handle) => {
                write!(f, "held download {} keeps its decided path", handle.0)
            }
            Self::List(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<ListError> for DownloadError {
    fn from(error: ListError) -> Self {
        Self::List(error)
    }
}

/// Continues a download CEF is waiting on. Dropping it unanswered cancels the download.
pub(crate) trait Proceed {
    fn proceed(self: Box<Self>, path: &Path);
}

/// CEF's controls for a download in progress.
pub(crate) trait Control {
    fn cancel(&self);
    fn pause(&self);
    fn resume(&self);
}

/// What `OnBeforeDownload` reports, plus the page that started it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Offer<'a> {
    pub handle: DownloadHandle,
    pub url: &'a str,
    pub original_url: &'a str,
    pub content_disposition: Option<&'a str>,
    pub suggested_name: Option<&'a str>,
    /// The tab's page, which Chromium falls back to when a download has no initiator.
    pub initiator: Option<&'a str>,
    pub started: NaiveDateTime,
    pub started_at: EpochMillis,
}

/// The URLs the insecure check reads, kept to run it again on a chosen name.
struct Delivery {
    url: String,
    redirects: Vec<String>,
    initiator: Option<String>,
}

impl Delivery {
    fn verdict(&self, name: &str, https_first_mode: bool) -> InsecureVerdict {
        let Ok(url) = Url::parse(&self.url) else {
            return InsecureVerdict::SilentBlock;
        };
        let redirects: Vec<Url> = self
            .redirects
            .iter()
            .filter_map(|url| Url::parse(url).ok())
            .collect();
        let initiator = self
            .initiator
            .as_deref()
            .and_then(|url| Url::parse(url).ok());
        insecure_verdict(
            name,
            InsecureSignals {
                download_url: &url,
                redirects: &redirects,
                initiator: initiator.as_ref(),
                trigger: TRIGGER,
                https_first_mode,
            },
        )
    }
}

/// CEF doesn't say how a download started; a link the person followed is the case with no
/// exemptions from the insecure checks.
const TRIGGER: Trigger = Trigger::Navigation {
    reload: false,
    typed: false,
    from_address_bar: false,
    forward_back: false,
    auto_toplevel: false,
    auto_bookmark: false,
    from_api: false,
};

/// CEF's `DownloadItem` can't report the gesture, the address bar or the referrer's history, so
/// none is assumed. File types Chromium allows on a user gesture are held instead.
const DANGER: DangerSignals = DangerSignals {
    has_user_gesture: false,
    from_address_bar: false,
    visited_referrer_before: false,
};

enum Stage {
    Ask { suggested: PathBuf },
    Held { id: DownloadId, path: PathBuf },
}

struct Waiting {
    proceed: Box<dyn Proceed>,
    browser: Weak<Slot>,
    stage: Stage,
    delivery: Delivery,
    started_at: EpochMillis,
}

/// A download CEF is saving. Its owner is known from the moment it is continued, before CEF's
/// first update brings the item's controls.
struct Item {
    control: Option<Rc<dyn Control>>,
    browser: Weak<Slot>,
    /// The shell cancelled it before its controls came; they cancel it when they do.
    cancel_pending: bool,
}

impl Item {
    fn new(browser: Weak<Slot>) -> Self {
        Self {
            control: None,
            browser,
            cancel_pending: false,
        }
    }
}

/// Work to do once the downloads borrow ends, since CEF may call back synchronously.
pub(crate) enum Act {
    Proceed(Box<dyn Proceed>, PathBuf),
    Cancel(Box<dyn Proceed>),
    CancelItem(Rc<dyn Control>),
    Pause(Rc<dyn Control>),
    Resume(Rc<dyn Control>),
    /// Lets go of an item's controls.
    Release(Rc<dyn Control>),
    Emit(Weak<Slot>, DownloadEvent),
}

#[derive(Default)]
pub(crate) struct Downloads {
    pub config: Option<DownloadConfig>,
    pub list: DownloadList,
    waiting: BTreeMap<DownloadHandle, Waiting>,
    items: BTreeMap<DownloadHandle, Item>,
    /// The handle of each download CEF is running. The list knows a download by its handle.
    handles: BTreeMap<CefDownload, DownloadHandle>,
    last_handle: u32,
    /// The request context that started each row, including rows whose browsers are gone.
    partitions: BTreeMap<DownloadId, Partition>,
    /// Where each incognito session last answered a save dialog. It lives as long as the session
    /// and never becomes the regular folder.
    prompt_folders: BTreeMap<IncognitoSession, PathBuf>,
    /// Addresses a tab's "Save … as" asked for, oldest first. Each request asks where to save
    /// once, so asking twice for one address asks for two downloads.
    save_as: Vec<(TabId, String)>,
}

fn same(browser: &Weak<Slot>, slot: &Slot) -> bool {
    std::ptr::eq(browser.as_ptr(), slot)
}

/// `path` with its folder resolved (`.`, `..` and symbolic links), so two spellings of one file
/// compare equal. A folder that can't be resolved, such as one that doesn't exist, is kept.
fn resolved(path: &Path) -> PathBuf {
    let folder = path
        .parent()
        .and_then(|folder| std::fs::canonicalize(folder).ok());
    match (folder, path.file_name()) {
        (Some(folder), Some(name)) => folder.join(name),
        _ => path.to_owned(),
    }
}

/// Whether a download in progress saves to `target`, however either path is spelled.
fn reserved(list: &DownloadList, target: &Path) -> bool {
    if list.reserves(target, Platform::current()) {
        return true;
    }
    let wanted = resolved(target);
    list.reserves(&wanted, Platform::current())
        || list.iter().any(|row| {
            matches!(row.state(), State::InProgress | State::Paused)
                && resolved(row.path()) == wanted
        })
}

impl Downloads {
    /// The handle of CEF's download, allocated the first time CEF names it.
    pub fn handle_for(&mut self, download: CefDownload) -> DownloadHandle {
        if let Some(handle) = self.handles.get(&download) {
            return *handle;
        }
        let handle = loop {
            self.last_handle = self.last_handle.wrapping_add(1);
            let handle = DownloadHandle(self.last_handle);
            let taken = handle.0 == 0
                || self.handles.values().any(|live| *live == handle)
                || self.waiting.contains_key(&handle)
                || self.items.contains_key(&handle)
                || self.list.id_for_cef(handle.0).is_some();
            if !taken {
                break handle;
            }
        };
        self.handles.insert(download, handle);
        handle
    }

    fn handle_partition(&self, handle: DownloadHandle) -> Option<&Partition> {
        self.handles
            .iter()
            .find(|(_, live)| **live == handle)
            .and_then(|((partition, _), _)| partition.as_ref())
    }

    fn handle_session(&self, handle: DownloadHandle) -> Option<IncognitoSession> {
        self.handle_partition(handle)
            .and_then(Partition::incognito_session)
    }

    /// Where a save dialog for `session` starts. An incognito session starts from the regular
    /// answer until it gives its own.
    fn prompt_folder(&self, session: Option<IncognitoSession>) -> Option<&Path> {
        session
            .and_then(|session| self.prompt_folders.get(&session))
            .map(PathBuf::as_path)
            .or_else(|| self.list.last_prompt_folder())
    }

    fn session(&self, event: &DownloadEvent) -> Option<IncognitoSession> {
        event
            .id()
            .and_then(|id| self.partitions.get(&id)?.incognito_session())
            .or_else(|| self.handle_session(event.handle()))
    }

    /// Drops the rows of an ended incognito session; the files stay on disk. A row still
    /// running stays until closing its browser has cancelled it.
    fn end_session(&mut self, session: IncognitoSession) {
        self.prompt_folders.remove(&session);
        let list = &mut self.list;
        self.partitions.retain(|id, partition| {
            partition.incognito_session() != Some(session)
                || matches!(list.remove(*id), Err(ListError::NotAllowed { .. }))
        });
    }

    /// The download ended, so its CEF id may name another download from now on.
    fn forget(&mut self, handle: DownloadHandle) {
        self.handles.retain(|_, live| *live != handle);
    }

    /// Parks a download until the shell answers. Handles are unique, so nothing is displaced;
    /// if something were, its callback would be released after the borrow, not inside it.
    fn wait(&mut self, handle: DownloadHandle, waiting: Waiting, acts: &mut Vec<Act>) {
        if let Some(displaced) = self.waiting.insert(handle, waiting) {
            acts.push(Act::Cancel(displaced.proceed));
        }
    }

    /// CEF is about to save it: from here on, closing its browser cancels it.
    fn saving(&mut self, handle: DownloadHandle, browser: Weak<Slot>) {
        self.items.entry(handle).or_insert(Item::new(browser));
    }

    pub fn before(
        &mut self,
        browser: Weak<Slot>,
        offer: Offer<'_>,
        proceed: Box<dyn Proceed>,
        exists: impl Fn(&Path) -> bool,
    ) -> Vec<Act> {
        let handle = offer.handle;
        let blocked = |downloads: &mut Self, browser: Weak<Slot>, proceed| {
            downloads.forget(handle);
            vec![
                Act::Cancel(proceed),
                Act::Emit(
                    browser,
                    DownloadEvent::Blocked {
                        handle,
                        url: offer.url.into(),
                    },
                ),
            ]
        };
        let (Some(config), Ok(download_url)) = (self.config.clone(), Url::parse(offer.url)) else {
            return blocked(self, browser, proceed);
        };
        let delivery = Delivery {
            url: offer.url.into(),
            redirects: (offer.original_url != offer.url && !offer.original_url.is_empty())
                .then(|| offer.original_url.to_owned())
                .into_iter()
                .collect(),
            initiator: offer.initiator.map(Into::into),
        };
        let redirects: Vec<Url> = delivery
            .redirects
            .iter()
            .filter_map(|url| Url::parse(url).ok())
            .collect();
        let initiator = offer.initiator.and_then(|url| Url::parse(url).ok());
        // Match only the address the tab actually requested (falling back to `url` only when
        // CEF leaves `original_url` empty), so a redirect from A to B never consumes B's Save As.
        let save_as = browser.upgrade().is_some_and(|slot| {
            let tab = slot.key.borrow().tab.clone();
            let requested = if !offer.original_url.is_empty() {
                offer.original_url
            } else {
                offer.url
            };
            self.save_as
                .iter()
                .position(|(asked, wanted)| *asked == tab && wanted == requested)
                .map(|index| self.save_as.remove(index))
                .is_some()
        });
        #[cfg(unix)]
        let max_name_length = flecha_downloads::name_max(&config.settings.folder);
        #[cfg(not(unix))]
        let max_name_length = None;
        let decision = decide(
            DownloadRequest {
                names: NameSources {
                    url: offer.url,
                    content_disposition: offer.content_disposition,
                    suggested_name: offer.suggested_name,
                },
                settings: &config.settings,
                last_prompt_folder: self.prompt_folder(self.handle_session(handle)),
                save_as,
                max_name_length,
                started: offer.started,
                danger: DANGER,
                insecure: InsecureSignals {
                    download_url: &download_url,
                    redirects: &redirects,
                    initiator: initiator.as_ref(),
                    trigger: TRIGGER,
                    https_first_mode: config.https_first_mode,
                },
                platform: Platform::current(),
            },
            &self.list,
            exists,
        );
        if decision.verdict == Verdict::Block {
            return blocked(self, browser, proceed);
        }
        let mut waiting = Waiting {
            proceed,
            browser: browser.clone(),
            stage: Stage::Ask {
                suggested: PathBuf::new(),
            },
            delivery,
            started_at: offer.started_at,
        };
        match decision.target {
            TargetDecision::Ask { suggested, reason } => {
                waiting.stage = Stage::Ask {
                    suggested: suggested.clone(),
                };
                let mut acts = Vec::new();
                self.wait(handle, waiting, &mut acts);
                acts.push(Act::Emit(
                    browser,
                    DownloadEvent::AskPath {
                        handle,
                        url: offer.url.into(),
                        suggested,
                        reason,
                    },
                ));
                acts
            }
            TargetDecision::Save(path) => {
                let warning = match decision.verdict {
                    Verdict::Hold(warning) => Some(warning),
                    Verdict::Proceed | Verdict::Block => None,
                };
                self.start(handle, waiting, path, warning)
            }
        }
    }

    /// Records the download at its authoritative path, then continues or holds it.
    fn start(
        &mut self,
        handle: DownloadHandle,
        mut waiting: Waiting,
        path: PathBuf,
        warning: Option<Warning>,
    ) -> Vec<Act> {
        let browser = waiting.browser.clone();
        // CEF takes the path as a string; a path that isn't UTF-8 never reaches it.
        if path.to_str().is_none() {
            self.forget(handle);
            let url = waiting.delivery.url.clone();
            return vec![
                Act::Cancel(waiting.proceed),
                Act::Emit(browser, DownloadEvent::Blocked { handle, url }),
            ];
        }
        let begun = self.list.begin(NewDownload {
            cef_id: handle.0,
            url: waiting.delivery.url.clone(),
            target: path.clone(),
            started_at: waiting.started_at,
            warning,
            // The engine passes the path to CEF itself, so no CEF dialog chooses it.
            prompted: false,
        });
        let Ok(id) = begun else {
            self.forget(handle);
            let url = waiting.delivery.url.clone();
            return vec![
                Act::Cancel(waiting.proceed),
                Act::Emit(browser, DownloadEvent::Blocked { handle, url }),
            ];
        };
        if let Some(partition) = self.handle_partition(handle).cloned() {
            self.partitions.insert(id, partition);
        }
        match warning {
            Some(warning) => {
                waiting.stage = Stage::Held { id, path };
                let mut acts = Vec::new();
                self.wait(handle, waiting, &mut acts);
                acts.push(Act::Emit(
                    browser,
                    DownloadEvent::Held {
                        handle,
                        id,
                        warning,
                    },
                ));
                acts
            }
            None => self.proceed(handle, id, waiting, path),
        }
    }

    /// Continues a recorded download. `Started` goes out first: CEF can report the download's
    /// first update from inside the continue call.
    fn proceed(
        &mut self,
        handle: DownloadHandle,
        id: DownloadId,
        waiting: Waiting,
        path: PathBuf,
    ) -> Vec<Act> {
        self.saving(handle, waiting.browser.clone());
        vec![
            Act::Emit(
                waiting.browser,
                DownloadEvent::Started {
                    handle,
                    id,
                    path: path.clone(),
                },
            ),
            Act::Proceed(waiting.proceed, path),
        ]
    }

    /// Saves where the person chose, or keeps a held download.
    pub fn accept(
        &mut self,
        handle: DownloadHandle,
        path: Option<PathBuf>,
    ) -> Result<Vec<Act>, DownloadError> {
        let waiting = self
            .waiting
            .remove(&handle)
            .ok_or(DownloadError::NotWaiting(handle))?;
        match &waiting.stage {
            Stage::Ask { suggested } => {
                let target = path.unwrap_or_else(|| suggested.clone());
                let name = target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                let Some(name) = name.filter(|_| target.is_absolute() && target.to_str().is_some())
                else {
                    self.waiting.insert(handle, waiting);
                    return Err(DownloadError::InvalidPath(target));
                };
                // Another prompt may have been answered with this path since this one was asked.
                if reserved(&self.list, &target) {
                    self.waiting.insert(handle, waiting);
                    return Err(DownloadError::PathInUse(target));
                }
                if let Some(folder) = target.parent() {
                    // The next save dialog of the same profile starts where this one was answered.
                    match self.handle_session(handle) {
                        Some(session) => {
                            self.prompt_folders.insert(session, folder.to_owned());
                        }
                        None => self.list.downloads_folder_changed(folder),
                    }
                }
                let https_first = self
                    .config
                    .as_ref()
                    .is_some_and(|config| config.https_first_mode);
                // Choosing the path approves the file type, as in Chromium, but the new name can
                // still make an insecure delivery one Chromium holds or blocks.
                match waiting.delivery.verdict(&name, https_first) {
                    InsecureVerdict::SilentBlock => {
                        self.forget(handle);
                        let url = waiting.delivery.url.clone();
                        let browser = waiting.browser.clone();
                        Ok(vec![
                            Act::Cancel(waiting.proceed),
                            Act::Emit(browser, DownloadEvent::Blocked { handle, url }),
                        ])
                    }
                    InsecureVerdict::Hold => {
                        Ok(self.start(handle, waiting, target, Some(Warning::Insecure)))
                    }
                    InsecureVerdict::Safe => Ok(self.start(handle, waiting, target, None)),
                }
            }
            Stage::Held { id, path: decided } => {
                let (id, decided) = (*id, decided.clone());
                if path.is_some_and(|path| path != decided) {
                    self.waiting.insert(handle, waiting);
                    return Err(DownloadError::KeepsDecidedPath(handle));
                }
                if let Err(error) = self.list.keep(id) {
                    self.waiting.insert(handle, waiting);
                    return Err(error.into());
                }
                Ok(self.proceed(handle, id, waiting, decided))
            }
        }
    }

    /// Discards a waiting download, or cancels one in progress.
    pub fn cancel(&mut self, handle: DownloadHandle) -> Result<Vec<Act>, DownloadError> {
        if let Some(waiting) = self.waiting.remove(&handle) {
            self.forget(handle);
            let id = match waiting.stage {
                Stage::Held { id, .. } => self.list.discard(id).ok().map(|_| id),
                Stage::Ask { .. } => None,
            };
            let mut acts = vec![
                Act::Cancel(waiting.proceed),
                Act::Emit(waiting.browser, DownloadEvent::Cancelled { handle, id }),
            ];
            // CEF may already have reported it; its later updates no longer find it.
            if let Some(Item {
                control: Some(control),
                ..
            }) = self.items.remove(&handle)
            {
                acts.push(Act::Release(control));
            }
            return Ok(acts);
        }
        let id = self
            .list
            .id_for_cef(handle.0)
            .ok_or(DownloadError::Unknown(handle))?;
        let item = self
            .items
            .get_mut(&handle)
            .ok_or(DownloadError::Unknown(handle))?;
        self.list.cancel(id)?;
        match item.control.clone() {
            Some(control) => Ok(vec![Act::CancelItem(control)]),
            // CEF brings an item's controls with its first update.
            None => {
                item.cancel_pending = true;
                Ok(Vec::new())
            }
        }
    }

    pub fn pause(&mut self, handle: DownloadHandle) -> Result<Vec<Act>, DownloadError> {
        let (id, control) = self.live(handle)?;
        self.list.pause(id)?;
        Ok(vec![Act::Pause(control)])
    }

    pub fn resume(&mut self, handle: DownloadHandle) -> Result<Vec<Act>, DownloadError> {
        let (id, control) = self.live(handle)?;
        self.list.resume(id)?;
        Ok(vec![Act::Resume(control)])
    }

    fn live(&self, handle: DownloadHandle) -> Result<(DownloadId, Rc<dyn Control>), DownloadError> {
        let id = self
            .list
            .id_for_cef(handle.0)
            .ok_or(DownloadError::Unknown(handle))?;
        // CEF brings an item's controls with its first update.
        let control = self
            .items
            .get(&handle)
            .and_then(|item| item.control.clone())
            .ok_or(DownloadError::Unknown(handle))?;
        Ok((id, control))
    }

    /// `OnDownloadUpdated`.
    pub fn updated(
        &mut self,
        browser: Weak<Slot>,
        download: CefDownload,
        report: Option<Snapshot<'_>>,
        control: Rc<dyn Control>,
    ) -> Vec<Act> {
        let mut acts = vec![Act::Release(control.clone())];
        let Some(snapshot) = report else {
            return acts;
        };
        let state = snapshot.state;
        let ended = matches!(state, SnapshotState::Complete | SnapshotState::Cancelled);
        // An ended download CEF never named before, or one the engine already let go, is done.
        let handle = if ended {
            self.handles.get(&download).copied()
        } else {
            Some(self.handle_for(download))
        };
        let Some(handle) = handle else {
            return acts;
        };
        let mut cancel = None;
        if ended {
            self.forget(handle);
            if let Some(Item {
                control: Some(held),
                ..
            }) = self.items.remove(&handle)
            {
                acts.push(Act::Release(held));
            }
        } else {
            let item = self
                .items
                .entry(handle)
                .or_insert_with(|| Item::new(browser.clone()));
            if let Some(previous) = item.control.replace(control.clone()) {
                acts.push(Act::Release(previous));
            }
            if std::mem::take(&mut item.cancel_pending) {
                cancel = Some(control);
            }
        }
        if state == SnapshotState::Cancelled
            && let Some(waiting) = self.waiting.remove(&handle)
        {
            let id = match waiting.stage {
                Stage::Held { id, .. } => self.list.discard(id).ok().map(|_| id),
                Stage::Ask { .. } => None,
            };
            acts.push(Act::Emit(
                browser.clone(),
                DownloadEvent::Cancelled { handle, id },
            ));
            acts.push(Act::Cancel(waiting.proceed));
            return acts;
        }
        // CEF may report a download before `OnBeforeDownload`; the list ignores it until then.
        if self.list.update(handle.0, snapshot).is_ok()
            && let Some(id) = self.list.id_for_cef(handle.0)
        {
            acts.push(Act::Emit(browser, DownloadEvent::Updated { handle, id }));
        }
        acts.extend(cancel.map(Act::CancelItem));
        acts
    }

    /// `browser` closed: cancel what it started, as CEF does with a destroyed browser's items.
    pub fn browser_closed(&mut self, browser: &Slot) -> Vec<Act> {
        let tab = browser.key.borrow().tab.clone();
        self.save_as.retain(|(asked, _)| *asked != tab);
        let mut acts = Vec::new();
        let waiting: Vec<_> = self
            .waiting
            .iter()
            .filter(|(_, waiting)| same(&waiting.browser, browser))
            .map(|(handle, _)| *handle)
            .collect();
        for handle in waiting {
            acts.extend(self.cancel(handle).unwrap_or_default());
        }
        let items: Vec<_> = self
            .items
            .iter()
            .filter(|(_, item)| same(&item.browser, browser))
            .map(|(handle, _)| *handle)
            .collect();
        for handle in items {
            let Some(item) = self.items.remove(&handle) else {
                continue;
            };
            self.forget(handle);
            let id = self.list.id_for_cef(handle.0);
            if let Some(entry) = id.and_then(|id| self.list.get(id))
                && matches!(
                    entry.state(),
                    State::InProgress | State::Paused | State::Interrupted
                )
            {
                let (received, total) = (entry.received(), entry.total());
                let snapshot = Snapshot {
                    state: SnapshotState::Cancelled,
                    received: i64::try_from(received).unwrap_or(i64::MAX),
                    total: i64::try_from(total).unwrap_or(i64::MAX),
                    full_path: Path::new(""),
                };
                let _ = self.list.update(handle.0, snapshot);
            }
            // Without controls yet, CEF cancels the destroyed browser's download itself.
            acts.extend(item.control.map(Act::CancelItem));
            acts.push(Act::Emit(
                item.browser,
                DownloadEvent::Cancelled { handle, id },
            ));
        }
        acts
    }

    /// Gives a shared-context CEF id the handle a test expects.
    #[cfg(test)]
    fn name(&mut self, id: u32, handle: DownloadHandle) {
        self.handles.insert((None, id), handle);
        self.last_handle = self.last_handle.max(handle.0);
    }

    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.waiting.is_empty() && self.items.is_empty() && self.handles.is_empty()
    }

    /// The next download of `url` from `tab` asks where to save, as Chromium's "Save … as" does.
    fn ask_where_to_save(&mut self, tab: TabId, url: &str) {
        self.save_as.push((tab, url.to_owned()));
    }
}

impl crate::Engine {
    /// Downloads `url` from the tab's page and asks where to save it ("Save link as…", "Save
    /// image as…", "Save as…"). A page saves as the file its address serves.
    pub fn save_as(&self, tab: &TabId, url: &str) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let host = self.host(tab).context("The page has closed")?;
        self.services()
            .downloads
            .borrow_mut()
            .ask_where_to_save(tab.clone(), url);
        host.start_download(Some(&CefString::from(url)));
        Ok(())
    }

    /// The folder and prompt settings downloads follow. `None` refuses every download.
    pub fn set_download_config(&self, config: Option<DownloadConfig>) {
        self.services().downloads.borrow_mut().config = config;
    }

    /// A copy of the sidebar's list. Change it through the engine so CEF and the list agree. A
    /// copy, so holding it never blocks a CEF callback or another engine call.
    pub fn downloads(&self) -> DownloadList {
        self.services().downloads.borrow().list.clone()
    }

    /// Installs the list loaded at startup (`flecha_downloads::persist::load`). Call it before
    /// the first download.
    pub fn restore_downloads(&self, list: DownloadList) {
        let mut downloads = self.services().downloads.borrow_mut();
        downloads.partitions.clear();
        downloads.list = list;
    }

    /// Drops a finished, cancelled or interrupted row. The file stays on disk.
    pub fn remove_download(&self, id: DownloadId) -> Result<(), DownloadError> {
        let mut downloads = self.services().downloads.borrow_mut();
        downloads.list.remove(id)?;
        downloads.partitions.remove(&id);
        Ok(())
    }

    /// `clearDownloads`: drops every row started after `since` that isn't progressing.
    pub fn clear_downloads(&self, since: EpochMillis) -> Vec<DownloadId> {
        let mut downloads = self.services().downloads.borrow_mut();
        let removed = downloads.list.clear(since);
        for id in &removed {
            downloads.partitions.remove(id);
        }
        removed
    }

    /// The handle of a download CEF is still running.
    pub fn download_handle(&self, id: DownloadId) -> Option<DownloadHandle> {
        let downloads = self.services().downloads.borrow();
        downloads
            .waiting
            .keys()
            .chain(downloads.items.keys())
            .copied()
            .find(|handle| downloads.list.id_for_cef(handle.0) == Some(id))
    }

    /// The incognito session that recorded the row, or `None` for a regular one.
    pub fn download_session(&self, id: DownloadId) -> Option<IncognitoSession> {
        self.services()
            .downloads
            .borrow()
            .partitions
            .get(&id)
            .and_then(Partition::incognito_session)
    }

    pub fn download_partition(&self, id: DownloadId) -> Option<Partition> {
        let downloads = self.services().downloads.borrow();
        downloads.list.get(id)?;
        Some(
            downloads
                .partitions
                .get(&id)
                .cloned()
                .unwrap_or(Partition::Shared),
        )
    }

    pub fn next_download_event(&mut self) -> Option<DownloadMessage> {
        self.adopt_popups();
        let (tab, event) = self.services().download_events.pop()?;
        let mut downloads = self.services().downloads.borrow_mut();
        let incognito = downloads.session(&event);
        if let Some(id) = event.id()
            && downloads.list.get(id).is_none()
        {
            downloads.partitions.remove(&id);
        }
        if let Some(session) = incognito
            && !self.contexts.contains_key(&Partition::Incognito(session))
        {
            downloads.end_session(session);
        }
        drop(downloads);
        Some(DownloadMessage {
            tab,
            incognito,
            event,
        })
    }

    /// Drops the download rows of an incognito session whose last window closed.
    pub(crate) fn end_download_session(&self, session: IncognitoSession) {
        self.services().downloads.borrow_mut().end_session(session);
    }

    /// Saves an asked download at `path` (`None` takes the suggestion), or keeps a held one
    /// (`path` must be `None` or its decided path).
    pub fn accept_download(
        &self,
        handle: DownloadHandle,
        path: Option<PathBuf>,
    ) -> Result<(), DownloadError> {
        self.download_command(|downloads| downloads.accept(handle, path))
    }

    /// Discards a waiting download, or cancels one in progress.
    pub fn cancel_download(&self, handle: DownloadHandle) -> Result<(), DownloadError> {
        self.download_command(|downloads| downloads.cancel(handle))
    }

    pub fn pause_download(&self, handle: DownloadHandle) -> Result<(), DownloadError> {
        self.download_command(|downloads| downloads.pause(handle))
    }

    pub fn resume_download(&self, handle: DownloadHandle) -> Result<(), DownloadError> {
        self.download_command(|downloads| downloads.resume(handle))
    }

    fn download_command(
        &self,
        command: impl FnOnce(&mut Downloads) -> Result<Vec<Act>, DownloadError>,
    ) -> Result<(), DownloadError> {
        let acts = command(&mut self.services().downloads.borrow_mut())?;
        run(self.services(), acts);
        Ok(())
    }
}

/// Runs `acts` once no downloads borrow is held.
pub(crate) fn run(services: &Services, acts: Vec<Act>) {
    for act in acts {
        match act {
            Act::Proceed(proceed, path) => proceed.proceed(&path),
            Act::Cancel(proceed) => drop(proceed),
            Act::CancelItem(control) => control.cancel(),
            Act::Pause(control) => control.pause(),
            Act::Resume(control) => control.resume(),
            Act::Release(control) => drop(control),
            Act::Emit(browser, event) => services.download_events.push(browser, event),
        }
    }
}

struct Before(BeforeDownloadCallback);

impl Proceed for Before {
    fn proceed(self: Box<Self>, path: &Path) {
        // A path CEF can't take as a string is dropped here, which cancels the download.
        if let Some(path) = path.to_str() {
            self.0.cont(Some(&CefString::from(path)), 0);
        }
    }
}

impl Control for DownloadItemCallback {
    fn cancel(&self) {
        ImplDownloadItemCallback::cancel(self);
    }
    fn pause(&self) {
        ImplDownloadItemCallback::pause(self);
    }
    fn resume(&self) {
        ImplDownloadItemCallback::resume(self);
    }
}

fn text(value: &CefStringUserfree) -> String {
    CefString::from(value).to_string()
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn epoch_millis() -> EpochMillis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| EpochMillis::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

wrap_download_handler! {
    pub(crate) struct Handler { slot: Rc<Slot> }
    impl DownloadHandler {
        fn can_download(
            &self, _browser: Option<&mut Browser>, _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self, browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>, callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return 0 };
            let url = text(&item.url());
            let original_url = text(&item.original_url());
            let disposition = non_empty(text(&item.content_disposition()));
            let suggested = suggested_name.map(ToString::to_string).and_then(non_empty);
            let initiator = browser
                .and_then(|browser| browser.main_frame())
                .map(|frame| text(&frame.url()));
            let offer = Offer {
                // `offered` names it.
                handle: DownloadHandle(0),
                url: &url,
                original_url: &original_url,
                content_disposition: disposition.as_deref(),
                suggested_name: suggested.as_deref(),
                initiator: initiator.as_deref(),
                started: chrono::Local::now().naive_local(),
                started_at: epoch_millis(),
            };
            offered(&self.slot, item.id(), offer, Box::new(Before(callback.clone())), Path::exists);
            1
        }

        fn on_download_updated(
            &self, _browser: Option<&mut Browser>, download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let (Some(item), Some(callback)) = (download_item, callback) else { return };
            let state = SnapshotState::from_cef(
                item.is_in_progress() != 0,
                item.is_paused() != 0,
                item.is_complete() != 0,
                item.is_canceled() != 0,
                item.is_interrupted() != 0,
                item.interrupt_reason().get_raw() as i32,
            );
            let full_path = PathBuf::from(text(&item.full_path()));
            let report = state.map(|state| Snapshot {
                state,
                received: item.received_bytes(),
                total: item.total_bytes(),
                full_path: &full_path,
            });
            reported(&self.slot, item.id(), report, Rc::new(callback.clone()));
        }
    }
}

/// `OnBeforeDownload` for CEF's download `id` in `slot`'s browser.
fn offered(
    slot: &Rc<Slot>,
    id: u32,
    mut offer: Offer<'_>,
    proceed: Box<dyn Proceed>,
    exists: impl Fn(&Path) -> bool,
) {
    let services = services::of(slot);
    let acts = {
        let mut downloads = services.downloads.borrow_mut();
        offer.handle = downloads.handle_for(cef_download(slot, id));
        downloads.before(Rc::downgrade(slot), offer, proceed, exists)
    };
    run(&services, acts);
}

/// `OnDownloadUpdated` for CEF's download `id` in `slot`'s browser.
fn reported(slot: &Rc<Slot>, id: u32, report: Option<Snapshot<'_>>, control: Rc<dyn Control>) {
    let services = services::of(slot);
    let acts = services.downloads.borrow_mut().updated(
        Rc::downgrade(slot),
        cef_download(slot, id),
        report,
        control,
    );
    run(&services, acts);
}

/// CEF numbers downloads per request context, which is the browser's partition.
fn cef_download(slot: &Slot, id: u32) -> CefDownload {
    (slot.partition.borrow().clone(), id)
}

pub(crate) fn handler(slot: Rc<Slot>) -> DownloadHandler {
    Handler::new(slot)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    type Log = Rc<RefCell<Vec<String>>>;

    struct FakeBefore {
        log: Log,
        done: bool,
    }

    impl Proceed for FakeBefore {
        fn proceed(mut self: Box<Self>, path: &Path) {
            self.done = true;
            self.log
                .borrow_mut()
                .push(format!("continue {}", path.display()));
        }
    }

    impl Drop for FakeBefore {
        fn drop(&mut self) {
            if !self.done {
                self.log.borrow_mut().push("cancelled".into());
            }
        }
    }

    struct FakeItem(Log);

    impl Control for FakeItem {
        fn cancel(&self) {
            self.0.borrow_mut().push("item cancel".into());
        }
        fn pause(&self) {
            self.0.borrow_mut().push("item pause".into());
        }
        fn resume(&self) {
            self.0.borrow_mut().push("item resume".into());
        }
    }

    /// An extension Chromium's file type policy allows only on a user gesture on this platform.
    const RISKY: &str = if cfg!(windows) { "exe" } else { "sh" };

    fn folder() -> PathBuf {
        PathBuf::from("/home/person/Downloads")
    }

    fn downloads(ask: bool) -> Downloads {
        Downloads {
            config: Some(DownloadConfig {
                settings: DownloadSettings {
                    folder: folder(),
                    ask_where_to_save: ask,
                },
                https_first_mode: false,
            }),
            ..Downloads::default()
        }
    }

    fn offer<'a>(handle: u32, url: &'a str, initiator: &'a str) -> Offer<'a> {
        Offer {
            handle: DownloadHandle(handle),
            url,
            original_url: url,
            content_disposition: None,
            suggested_name: None,
            initiator: Some(initiator),
            started: chrono::NaiveDate::from_ymd_opt(2026, 11, 24)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
            started_at: 1_000,
        }
    }

    fn report(
        state: SnapshotState,
        received: i64,
        total: i64,
        full_path: &Path,
    ) -> Option<Snapshot<'_>> {
        Some(Snapshot {
            state,
            received,
            total,
            full_path,
        })
    }

    /// Runs the acts against the fakes and returns the events.
    fn settle(acts: Vec<Act>) -> Vec<DownloadEvent> {
        let mut events = Vec::new();
        for act in acts {
            match act {
                Act::Proceed(proceed, path) => proceed.proceed(&path),
                Act::Cancel(proceed) => drop(proceed),
                Act::CancelItem(control) => control.cancel(),
                Act::Pause(control) => control.pause(),
                Act::Resume(control) => control.resume(),
                Act::Release(control) => drop(control),
                Act::Emit(_, event) => events.push(event),
            }
        }
        events
    }

    fn before(
        downloads: &mut Downloads,
        log: &Log,
        offer: Offer<'_>,
        browser: Weak<Slot>,
    ) -> Vec<DownloadEvent> {
        let proceed = Box::new(FakeBefore {
            log: log.clone(),
            done: false,
        });
        downloads.name(offer.handle.0, offer.handle);
        settle(downloads.before(browser, offer, proceed, |_| false))
    }

    #[test]
    fn save_as_asks_where_once_for_its_own_tab() {
        let mut downloads = downloads(false);
        let log = Log::default();
        let slots = crate::tests::slots(&["page", "other"]);
        let url = "https://files.example/report.pdf";
        downloads.ask_where_to_save(TabId::new("page"), url);

        // Another tab's download of the same address saves without asking.
        let events = before(
            &mut downloads,
            &log,
            offer(1, url, "https://files.example/"),
            Rc::downgrade(&slots[1]),
        );
        assert!(matches!(events[..], [DownloadEvent::Started { .. }]));

        let events = before(
            &mut downloads,
            &log,
            offer(2, url, "https://files.example/"),
            Rc::downgrade(&slots[0]),
        );
        assert!(
            matches!(
                &events[..],
                [DownloadEvent::AskPath {
                    handle: DownloadHandle(2),
                    reason: AskReason::SaveAs,
                    ..
                }]
            ),
            "{events:?}"
        );

        // The request is used up: the next download of the address saves without asking.
        let events = before(
            &mut downloads,
            &log,
            offer(3, url, "https://files.example/"),
            Rc::downgrade(&slots[0]),
        );
        assert!(matches!(events[..], [DownloadEvent::Started { .. }]));

        // A closed browser's pending request goes with it.
        downloads.ask_where_to_save(TabId::new("other"), url);
        settle(downloads.browser_closed(&slots[1]));
        assert!(downloads.save_as.is_empty());
    }

    fn asks_where(events: &[DownloadEvent]) -> bool {
        matches!(
            events,
            [DownloadEvent::AskPath {
                reason: AskReason::SaveAs,
                ..
            }]
        )
    }

    #[test]
    fn each_save_as_request_asks_once_and_keeps_to_the_address_it_named() {
        let mut downloads = downloads(false);
        let log = Log::default();
        let slots = crate::tests::slots(&["page"]);
        let page = TabId::new("page");
        let report = "https://files.example/report.pdf";
        let mirror = "https://mirror.example/report.pdf";

        // Asking twice for one address asks for both downloads.
        downloads.ask_where_to_save(page.clone(), report);
        downloads.ask_where_to_save(page.clone(), report);
        for handle in [1, 2] {
            let events = before(
                &mut downloads,
                &log,
                offer(handle, report, "https://files.example/"),
                Rc::downgrade(&slots[0]),
            );
            assert!(asks_where(&events), "{events:?}");
        }

        // A download of `mirror` that redirected to `report` uses the request for `mirror`,
        // and leaves the one for `report` to the download that names it.
        downloads.ask_where_to_save(page.clone(), report);
        downloads.ask_where_to_save(page.clone(), mirror);
        let redirected = Offer {
            original_url: mirror,
            ..offer(3, report, "https://files.example/")
        };
        let events = before(&mut downloads, &log, redirected, Rc::downgrade(&slots[0]));
        assert!(asks_where(&events), "{events:?}");
        assert_eq!(downloads.save_as, [(page.clone(), report.to_owned())]);
        let events = before(
            &mut downloads,
            &log,
            offer(4, report, "https://files.example/"),
            Rc::downgrade(&slots[0]),
        );
        assert!(asks_where(&events), "{events:?}");
        assert!(downloads.save_as.is_empty());
    }

    #[test]
    fn a_safe_download_continues_at_once_to_its_decided_path() {
        let mut downloads = downloads(false);
        let log = Log::default();
        let events = before(
            &mut downloads,
            &log,
            offer(
                1,
                "https://files.example/report.pdf",
                "https://files.example/",
            ),
            Weak::new(),
        );
        let path = folder().join("report.pdf");
        assert_eq!(
            events,
            [DownloadEvent::Started {
                handle: DownloadHandle(1),
                id: 1,
                path: path.clone()
            }]
        );
        assert_eq!(*log.borrow(), [format!("continue {}", path.display())]);

        // CEF's intermediate name never replaces the decided path.
        let item = Rc::new(FakeItem(log.clone()));
        let intermediate = folder().join("Unconfirmed 123.crdownload");
        settle(downloads.updated(
            Weak::new(),
            (None, 1),
            report(SnapshotState::InProgress, 10, 100, &intermediate),
            item.clone(),
        ));
        assert_eq!(downloads.list.get(1).unwrap().path(), path);
        assert!(downloads.list.reserves(&path, Platform::current()));
        settle(downloads.updated(
            Weak::new(),
            (None, 1),
            report(SnapshotState::Complete, 100, 100, &path),
            item,
        ));
        assert_eq!(downloads.list.get(1).unwrap().state(), State::Complete);
        assert!(downloads.is_idle());
    }

    #[test]
    fn a_held_download_waits_for_keep_or_discard() {
        let mut downloads = downloads(false);
        let log = Log::default();
        // A type Chromium allows only on a user gesture CEF could vouch for.
        let events = before(
            &mut downloads,
            &log,
            offer(
                2,
                &format!("https://files.example/setup.{RISKY}"),
                "https://files.example/",
            ),
            Weak::new(),
        );
        assert_eq!(
            events,
            [DownloadEvent::Held {
                handle: DownloadHandle(2),
                id: 1,
                warning: Warning::Dangerous
            }]
        );
        assert!(log.borrow().is_empty(), "nothing saved while held");

        assert_eq!(
            downloads
                .accept(DownloadHandle(2), Some(folder().join("renamed.txt")))
                .err(),
            Some(DownloadError::KeepsDecidedPath(DownloadHandle(2)))
        );
        let events = settle(downloads.accept(DownloadHandle(2), None).unwrap());
        let path = folder().join(format!("setup.{RISKY}"));
        assert_eq!(
            events,
            [DownloadEvent::Started {
                handle: DownloadHandle(2),
                id: 1,
                path: path.clone()
            }]
        );
        assert_eq!(*log.borrow(), [format!("continue {}", path.display())]);
        assert_eq!(downloads.list.get(1).unwrap().warning(), None);

        // Discarding a second one cancels it and drops its row.
        before(
            &mut downloads,
            &log,
            offer(
                3,
                &format!("https://files.example/other.{RISKY}"),
                "https://files.example/",
            ),
            Weak::new(),
        );
        let events = settle(downloads.cancel(DownloadHandle(3)).unwrap());
        assert_eq!(
            events,
            [DownloadEvent::Cancelled {
                handle: DownloadHandle(3),
                id: Some(2)
            }]
        );
        assert_eq!(log.borrow().last().map(String::as_str), Some("cancelled"));
        assert!(downloads.list.get(2).is_none());
    }

    #[test]
    fn a_chosen_name_is_checked_again_for_insecure_delivery() {
        let mut downloads = downloads(true);
        let log = Log::default();
        // Plain HTTP from an HTTP page: a .txt is safe, an executable is held.
        let events = before(
            &mut downloads,
            &log,
            offer(4, "http://files.example/notes.txt", "http://files.example/"),
            Weak::new(),
        );
        let suggested = folder().join("notes.txt");
        assert_eq!(
            events,
            [DownloadEvent::AskPath {
                handle: DownloadHandle(4),
                url: "http://files.example/notes.txt".into(),
                suggested: suggested.clone(),
                reason: AskReason::Preference,
            }]
        );
        assert_eq!(
            downloads
                .accept(DownloadHandle(4), Some("relative.txt".into()))
                .err(),
            Some(DownloadError::InvalidPath("relative.txt".into()))
        );
        let chosen = PathBuf::from("/home/person/Desktop/notes.exe");
        let events = settle(
            downloads
                .accept(DownloadHandle(4), Some(chosen.clone()))
                .unwrap(),
        );
        assert_eq!(
            events,
            [DownloadEvent::Held {
                handle: DownloadHandle(4),
                id: 1,
                warning: Warning::Insecure
            }]
        );
        assert!(log.borrow().is_empty());
        assert_eq!(downloads.list.get(1).unwrap().path(), chosen);
        assert_eq!(
            downloads.list.last_prompt_folder(),
            Some(Path::new("/home/person/Desktop"))
        );

        let events = settle(downloads.accept(DownloadHandle(4), None).unwrap());
        assert_eq!(
            events,
            [DownloadEvent::Started {
                handle: DownloadHandle(4),
                id: 1,
                path: chosen.clone()
            }]
        );
    }

    #[test]
    fn mixed_content_downloads_and_missing_settings_are_refused() {
        let log = Log::default();
        let mut downloads = downloads(false);
        let events = before(
            &mut downloads,
            &log,
            offer(5, "http://files.example/setup.zip", "https://shop.example/"),
            Weak::new(),
        );
        assert!(
            matches!(events[..], [DownloadEvent::Blocked { .. }]),
            "{events:?}"
        );
        assert_eq!(*log.borrow(), ["cancelled"]);

        let mut unconfigured = Downloads::default();
        let events = before(
            &mut unconfigured,
            &log,
            offer(6, "https://files.example/a.pdf", "https://files.example/"),
            Weak::new(),
        );
        assert!(matches!(events[..], [DownloadEvent::Blocked { .. }]));
        assert!(unconfigured.list.iter().next().is_none());
    }

    #[test]
    fn closing_the_browser_cancels_everything_it_started() {
        let slot = crate::tests::slot("closing");
        let other = crate::tests::slot("other");
        let log = Log::default();
        let mut downloads = downloads(false);
        let browser = Rc::downgrade(&slot);
        before(
            &mut downloads,
            &log,
            offer(
                7,
                &format!("https://files.example/run.{RISKY}"),
                "https://files.example/",
            ),
            browser.clone(),
        );
        before(
            &mut downloads,
            &log,
            offer(8, "https://files.example/a.pdf", "https://files.example/"),
            browser.clone(),
        );
        before(
            &mut downloads,
            &log,
            offer(9, "https://files.example/b.pdf", "https://files.example/"),
            Rc::downgrade(&other),
        );
        for handle in [8, 9] {
            let owner = if handle == 8 {
                browser.clone()
            } else {
                Rc::downgrade(&other)
            };
            let item = Rc::new(FakeItem(log.clone()));
            settle(downloads.updated(
                owner,
                (None, handle),
                report(SnapshotState::InProgress, 1, 10, Path::new("")),
                item,
            ));
        }
        log.borrow_mut().clear();

        let events = settle(downloads.browser_closed(&slot));
        assert_eq!(
            events,
            [
                DownloadEvent::Cancelled {
                    handle: DownloadHandle(7),
                    id: Some(1)
                },
                DownloadEvent::Cancelled {
                    handle: DownloadHandle(8),
                    id: Some(2)
                },
            ]
        );
        assert_eq!(*log.borrow(), ["cancelled", "item cancel"]);
        assert!(
            downloads.list.get(1).is_none(),
            "the held download is discarded"
        );
        assert_eq!(downloads.list.get(2).unwrap().state(), State::Cancelled);
        assert_eq!(
            downloads.list.get(3).unwrap().state(),
            State::InProgress,
            "other tabs keep theirs"
        );
        assert!(downloads.accept(DownloadHandle(7), None).is_err());

        // Pausing and resuming go through the list's rules to CEF's item callback.
        log.borrow_mut().clear();
        settle(downloads.pause(DownloadHandle(9)).unwrap());
        settle(downloads.updated(
            Rc::downgrade(&other),
            (None, 9),
            report(SnapshotState::Paused, 1, 10, Path::new("")),
            Rc::new(FakeItem(log.clone())),
        ));
        settle(downloads.resume(DownloadHandle(9)).unwrap());
        assert_eq!(*log.borrow(), ["item pause", "item resume"]);
        assert!(downloads.resume(DownloadHandle(8)).is_err());
    }

    #[test]
    fn cef_destroying_a_browser_denies_its_permissions_and_cancels_its_downloads_and_cdp_calls() {
        use crate::permissions::{self, tests as asks};

        let slot = crate::tests::slot("destroyed");
        slot.identifier.set(Some(4));
        let services = crate::services::of(&slot);
        let log = Log::default();
        let mut held = downloads(false);
        let risky = format!("https://files.example/run.{RISKY}");
        let events = before(
            &mut held,
            &log,
            offer(1, &risky, "https://files.example/"),
            Rc::downgrade(&slot),
        );
        assert!(
            matches!(events[..], [DownloadEvent::Held { .. }]),
            "{events:?}"
        );
        services.downloads.replace(held);

        let answers = asks::Log::default();
        let mut ask = asks::camera_ask("", "https://meet.example/");
        ask.queue = permissions::queue_id(4);
        permissions::request(&services, ask, asks::recorder("held", &answers));
        assert!(
            answers.borrow().is_empty(),
            "the prompt waits for the banner"
        );

        let pending = (Rc::downgrade(&slot), crate::devtools::Internal::Ignore);
        services.devtools_internal.borrow_mut().insert(1, pending);

        slot.destroyed();
        assert!(
            services.devtools_internal.borrow().is_empty(),
            "its CDP replies never come"
        );
        assert_eq!(
            *answers.borrow(),
            [("held", Some(flecha_permissions::Verdict::Block))]
        );
        assert_eq!(*log.borrow(), ["cancelled"]);
        assert!(
            services.downloads.borrow().list.get(1).is_none(),
            "the held download is discarded"
        );
        assert!(services.downloads.borrow().waiting.is_empty());
    }

    /// A CEF callback that calls straight back into the download handler when it is answered or
    /// dropped, as CEF does: it panics if the engine still holds the downloads borrow.
    struct Reentrant {
        services: Rc<Services>,
        log: Log,
        done: bool,
    }

    impl Reentrant {
        fn boxed(services: &Rc<Services>, log: &Log) -> Box<Self> {
            Box::new(Self {
                services: services.clone(),
                log: log.clone(),
                done: false,
            })
        }

        fn reenter(&self) {
            drop(self.services.downloads.borrow_mut());
        }
    }

    impl Proceed for Reentrant {
        fn proceed(mut self: Box<Self>, path: &Path) {
            self.reenter();
            self.done = true;
            self.log
                .borrow_mut()
                .push(format!("continue {}", path.display()));
        }
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            if !self.done {
                self.reenter();
                self.log.borrow_mut().push("cancelled".into());
            }
        }
    }

    struct ReentrantItem(Rc<Services>);

    impl Control for ReentrantItem {
        fn cancel(&self) {
            drop(self.0.downloads.borrow_mut());
        }
        fn pause(&self) {}
        fn resume(&self) {}
    }

    impl Drop for ReentrantItem {
        fn drop(&mut self) {
            drop(self.0.downloads.borrow_mut());
        }
    }

    fn drain(services: &Services) -> Vec<DownloadEvent> {
        std::iter::from_fn(|| services.download_events.pop())
            .map(|(_, event)| event)
            .collect()
    }

    fn ask_handle(event: &DownloadEvent) -> DownloadHandle {
        match event {
            DownloadEvent::AskPath { handle, .. } => *handle,
            other => panic!("expected AskPath, got {other:?}"),
        }
    }

    /// Runs a shell command the way [`crate::Engine`] does.
    fn command(
        services: &Services,
        command: impl FnOnce(&mut Downloads) -> Result<Vec<Act>, DownloadError>,
    ) -> Result<(), DownloadError> {
        let acts = command(&mut services.downloads.borrow_mut())?;
        run(services, acts);
        Ok(())
    }

    #[test]
    fn shared_and_isolated_downloads_with_one_cef_id_stay_apart() {
        let [shared, isolated] = <[_; 2]>::try_from(crate::tests::slots(&["shared", "isolated"]))
            .unwrap_or_else(|_| unreachable!());
        shared.partition.replace(Some(crate::Partition::Shared));
        let space = flecha_core::SpaceId::new("work");
        isolated
            .partition
            .replace(Some(crate::Partition::Isolated(space)));
        let services = crate::services::of(&shared);
        services.downloads.borrow_mut().config = downloads(true).config;
        let log = Log::default();

        // CEF numbers downloads per request context: both of these are its download 1.
        let url = "https://files.example/a.txt";
        offered(
            &shared,
            1,
            offer(0, url, url),
            Reentrant::boxed(&services, &log),
            |_| false,
        );
        offered(
            &isolated,
            1,
            offer(0, url, url),
            Reentrant::boxed(&services, &log),
            |_| false,
        );
        let events = drain(&services);
        let (first, second) = (ask_handle(&events[0]), ask_handle(&events[1]));
        assert_ne!(first, second);
        assert!(
            log.borrow().is_empty(),
            "the second prompt never cancels the first"
        );

        // Both suggested the same name; the first answer reserves it.
        let picked = folder().join("picked.txt");
        command(&services, |downloads| {
            downloads.accept(first, Some(picked.clone()))
        })
        .unwrap();
        assert_eq!(*log.borrow(), [format!("continue {}", picked.display())]);
        assert_eq!(
            command(&services, |downloads| downloads
                .accept(second, Some(picked.clone()))),
            Err(DownloadError::PathInUse(picked.clone()))
        );

        // Updates find their own download by context, and run outside the borrow.
        let item = || Rc::new(ReentrantItem(services.clone()));
        reported(
            &shared,
            1,
            report(SnapshotState::InProgress, 1, 10, &picked),
            item(),
        );
        reported(
            &isolated,
            1,
            report(SnapshotState::InProgress, 1, 10, Path::new("")),
            item(),
        );
        let started: Vec<_> = drain(&services);
        assert!(
            matches!(started[..], [DownloadEvent::Started { handle, .. }, DownloadEvent::Updated { handle: updated, .. }] if handle == first && updated == first),
            "{started:?}"
        );
        command(&services, |downloads| downloads.cancel(second)).unwrap();
        assert_eq!(log.borrow().last().map(String::as_str), Some("cancelled"));
        reported(
            &isolated,
            1,
            report(SnapshotState::Cancelled, 1, 10, Path::new("")),
            item(),
        );
        reported(
            &shared,
            1,
            report(SnapshotState::Complete, 10, 10, &picked),
            item(),
        );
        let list = services.downloads.borrow().list.clone();
        let row = list.iter().next().unwrap();
        assert_eq!(
            (row.state(), row.path()),
            (State::Complete, picked.as_path())
        );
        assert!(services.downloads.borrow().is_idle());
    }

    #[test]
    fn an_incognito_session_lists_its_own_downloads_until_it_ends() {
        let [regular, private] = <[_; 2]>::try_from(crate::tests::slots(&["regular", "private"]))
            .unwrap_or_else(|_| unreachable!());
        let session = IncognitoSession::new(1);
        regular.partition.replace(Some(Partition::Shared));
        private
            .partition
            .replace(Some(Partition::Incognito(session)));
        let services = crate::services::of(&regular);
        services.downloads.borrow_mut().config = downloads(false).config;
        let log = Log::default();
        let finished = "https://files.example/finished.txt";
        let running = "https://files.example/running.txt";
        for (slot, id, url) in [
            (&regular, 1, finished),
            (&private, 1, finished),
            (&private, 2, running),
        ] {
            offered(
                slot,
                id,
                offer(0, url, url),
                Reentrant::boxed(&services, &log),
                |_| false,
            );
        }
        let started = drain(&services);
        let sessions: Vec<_> = started
            .iter()
            .map(|event| services.downloads.borrow().session(event))
            .collect();
        assert_eq!(sessions, [None, Some(session), Some(session)]);
        let [regular_id, private_id, running_id] = <[_; 3]>::try_from(
            started
                .iter()
                .map(|event| event.id().unwrap())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let path = |id| {
            let downloads = services.downloads.borrow();
            downloads.list.get(id).unwrap().path().to_owned()
        };
        let item = || Rc::new(ReentrantItem(services.clone()));
        for (slot, cef_id, id, state) in [
            (&regular, 1, regular_id, SnapshotState::Complete),
            (&private, 1, private_id, SnapshotState::Complete),
            (&private, 2, running_id, SnapshotState::InProgress),
        ] {
            reported(slot, cef_id, report(state, 10, 10, &path(id)), item());
        }

        // The session ends while one download still runs. The finished row goes at once; the
        // running one stays until closing its browser has cancelled it.
        services.downloads.borrow_mut().end_session(session);
        let rows = |services: &Services| -> Vec<DownloadId> {
            services
                .downloads
                .borrow()
                .list
                .iter()
                .map(flecha_downloads::Download::id)
                .collect()
        };
        assert_eq!(rows(&services), [regular_id, running_id]);
        reported(
            &private,
            2,
            report(SnapshotState::Cancelled, 10, 10, &path(running_id)),
            item(),
        );
        let cancelled = drain(&services)
            .into_iter()
            .rfind(|event| event.id() == Some(running_id))
            .unwrap();
        assert_eq!(
            services.downloads.borrow().session(&cancelled),
            Some(session),
            "a cancelled row still names its session"
        );
        services.downloads.borrow_mut().end_session(session);
        assert_eq!(rows(&services), [regular_id]);
        assert!(
            services
                .downloads
                .borrow()
                .partitions
                .values()
                .all(|partition| !partition.is_incognito())
        );
    }

    #[test]
    fn an_incognito_save_dialog_answer_never_becomes_the_regular_folder() {
        let [regular, private] = <[_; 2]>::try_from(crate::tests::slots(&["regular", "private"]))
            .unwrap_or_else(|_| unreachable!());
        let session = IncognitoSession::new(1);
        regular.partition.replace(Some(Partition::Shared));
        private
            .partition
            .replace(Some(Partition::Incognito(session)));
        let services = crate::services::of(&regular);
        services.downloads.borrow_mut().config = downloads(true).config;
        let log = Log::default();
        let url = "https://files.example/a.txt";
        let suggested = |slot: &Rc<Slot>, id| {
            offered(
                slot,
                id,
                offer(0, url, url),
                Reentrant::boxed(&services, &log),
                |_| false,
            );
            match drain(&services).pop() {
                Some(DownloadEvent::AskPath {
                    handle, suggested, ..
                }) => (handle, suggested),
                other => panic!("expected AskPath, got {other:?}"),
            }
        };

        let (handle, _) = suggested(&private, 1);
        let chosen = folder().join("private-choice");
        command(&services, |downloads| {
            downloads.accept(handle, Some(chosen.join("a.txt")))
        })
        .unwrap();
        assert_eq!(
            suggested(&private, 2).1.parent(),
            Some(chosen.as_path()),
            "the session starts its next dialog where it answered the last one"
        );
        assert_eq!(
            suggested(&regular, 1).1.parent(),
            Some(folder().as_path()),
            "a regular dialog still starts in the regular default folder"
        );
        assert_eq!(services.downloads.borrow().list.last_prompt_folder(), None);

        services.downloads.borrow_mut().end_session(session);
        assert!(services.downloads.borrow().prompt_folders.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_chosen_path_cef_cannot_take_keeps_the_prompt_open() {
        use std::os::unix::ffi::OsStrExt as _;

        let mut downloads = downloads(true);
        let log = Log::default();
        let url = "https://files.example/a.txt";
        before(&mut downloads, &log, offer(1, url, url), Weak::new());
        let invalid = folder().join(std::ffi::OsStr::from_bytes(b"\xff.txt"));
        assert_eq!(
            downloads
                .accept(DownloadHandle(1), Some(invalid.clone()))
                .err(),
            Some(DownloadError::InvalidPath(invalid))
        );
        assert!(log.borrow().is_empty());
        let events = settle(downloads.accept(DownloadHandle(1), None).unwrap());
        assert!(
            matches!(events[..], [DownloadEvent::Started { .. }]),
            "{events:?}"
        );
    }

    #[test]
    fn closing_the_browser_before_cefs_first_update_cancels_the_download() {
        let slot = crate::tests::slot("closing");
        let services = crate::services::of(&slot);
        services.downloads.borrow_mut().config = downloads(false).config;
        let log = Log::default();
        let url = "https://files.example/a.pdf";
        offered(
            &slot,
            3,
            offer(0, url, url),
            Reentrant::boxed(&services, &log),
            |_| false,
        );
        let Some(DownloadEvent::Started { handle, id, .. }) = drain(&services).pop() else {
            panic!("a safe download starts at once");
        };

        slot.destroyed();
        assert_eq!(
            drain(&services),
            [DownloadEvent::Cancelled {
                handle,
                id: Some(id)
            }]
        );
        let list = services.downloads.borrow().list.clone();
        assert_eq!(list.get(id).unwrap().state(), State::Cancelled);
        assert!(services.downloads.borrow().is_idle());
    }

    /// Continues the way CEF can: its first update for the download arrives from inside the
    /// continue call.
    struct UpdatesAtOnce {
        slot: Rc<Slot>,
        id: u32,
        log: Log,
    }

    impl Proceed for UpdatesAtOnce {
        fn proceed(self: Box<Self>, path: &Path) {
            let item = Rc::new(FakeItem(self.log.clone()));
            reported(
                &self.slot,
                self.id,
                report(SnapshotState::InProgress, 1, 10, path),
                item,
            );
        }
    }

    #[test]
    fn started_reaches_the_shell_before_an_update_cef_sends_while_continuing() {
        let slot = crate::tests::slot("tab");
        let services = crate::services::of(&slot);
        let log = Log::default();
        let url = "https://files.example/a.pdf";
        let updates_at_once = |id| {
            Box::new(UpdatesAtOnce {
                slot: slot.clone(),
                id,
                log: log.clone(),
            })
        };
        let started_then_updated = |events: Vec<DownloadEvent>| {
            assert!(
                matches!(events[..], [DownloadEvent::Started { handle, .. }, DownloadEvent::Updated { handle: updated, .. }] if handle == updated),
                "{events:?}"
            );
        };

        // Saved at once to the decided path.
        services.downloads.borrow_mut().config = downloads(false).config;
        offered(&slot, 1, offer(0, url, url), updates_at_once(1), |_| false);
        started_then_updated(drain(&services));

        // Saved where the person chose.
        services.downloads.borrow_mut().config = downloads(true).config;
        offered(&slot, 2, offer(0, url, url), updates_at_once(2), |_| false);
        let handle = ask_handle(&drain(&services)[0]);
        let chosen = folder().join("chosen.pdf");
        command(&services, |downloads| {
            downloads.accept(handle, Some(chosen))
        })
        .unwrap();
        started_then_updated(drain(&services));
    }

    #[test]
    fn cancelling_before_cefs_first_update_cancels_once_the_controls_come() {
        let slot = crate::tests::slot("tab");
        let services = crate::services::of(&slot);
        services.downloads.borrow_mut().config = downloads(false).config;
        let log = Log::default();
        let url = "https://files.example/a.pdf";
        offered(
            &slot,
            3,
            offer(0, url, url),
            Reentrant::boxed(&services, &log),
            |_| false,
        );
        let Some(DownloadEvent::Started { handle, id, path }) = drain(&services).pop() else {
            panic!("a safe download starts at once");
        };

        command(&services, |downloads| downloads.cancel(handle)).unwrap();
        let cancels = || {
            log.borrow()
                .iter()
                .filter(|line| *line == "item cancel")
                .count()
        };
        assert_eq!(
            cancels(),
            0,
            "nothing to cancel with before CEF's first update"
        );

        let item = || Rc::new(FakeItem(log.clone()));
        reported(
            &slot,
            3,
            report(SnapshotState::InProgress, 1, 10, &path),
            item(),
        );
        assert_eq!(cancels(), 1, "the first controls cancel it");
        reported(
            &slot,
            3,
            report(SnapshotState::InProgress, 2, 10, &path),
            item(),
        );
        assert_eq!(cancels(), 1, "and only once");
        reported(
            &slot,
            3,
            report(SnapshotState::Cancelled, 2, 10, &path),
            item(),
        );
        let list = services.downloads.borrow().list.clone();
        assert_eq!(list.get(id).unwrap().state(), State::Cancelled);
        assert!(services.downloads.borrow().is_idle());
    }

    #[test]
    fn a_path_in_use_is_found_however_it_is_spelled() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Downloads");
        std::fs::create_dir(&folder).unwrap();
        let mut downloads = downloads(true);
        let log = Log::default();
        let url = "https://files.example/a.txt";
        before(&mut downloads, &log, offer(1, url, url), Weak::new());
        before(&mut downloads, &log, offer(2, url, url), Weak::new());
        settle(
            downloads
                .accept(DownloadHandle(1), Some(folder.join("picked.txt")))
                .unwrap(),
        );

        let mut spellings = vec![folder.join("..").join("Downloads").join("picked.txt")];
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&folder, &link).unwrap();
            spellings.push(link.join("picked.txt"));
        }
        for spelling in spellings {
            assert_eq!(
                downloads
                    .accept(DownloadHandle(2), Some(spelling.clone()))
                    .err(),
                Some(DownloadError::PathInUse(spelling))
            );
        }
        let other = folder.join("other.txt");
        settle(
            downloads
                .accept(DownloadHandle(2), Some(other.clone()))
                .unwrap(),
        );
        assert_eq!(
            log.borrow().last(),
            Some(&format!("continue {}", other.display()))
        );
    }
}
