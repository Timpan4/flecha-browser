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

/// A download for this CEF session: CEF's download id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DownloadHandle(pub u32);

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadMessage {
    /// The tab that started it, while the engine still owns that tab's browser.
    pub tab: Option<TabId>,
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
    /// A chosen path must be absolute and name a file.
    InvalidPath(PathBuf),
    /// A held download is kept at its decided path. Choosing another path is asked first.
    KeepsDecidedPath(DownloadHandle),
    List(ListError),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWaiting(handle) => {
                write!(f, "download {} isn't waiting for an answer", handle.0)
            }
            Self::Unknown(handle) => write!(f, "no live download {}", handle.0),
            Self::InvalidPath(path) => write!(f, "{} is not an absolute file path", path.display()),
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

struct Item {
    control: Rc<dyn Control>,
    browser: Weak<Slot>,
}

/// Work to do once the downloads borrow ends, since CEF may call back synchronously.
pub(crate) enum Act {
    Proceed(Box<dyn Proceed>, PathBuf),
    Cancel(Box<dyn Proceed>),
    CancelItem(Rc<dyn Control>),
    Pause(Rc<dyn Control>),
    Resume(Rc<dyn Control>),
    Emit(Weak<Slot>, DownloadEvent),
}

#[derive(Default)]
pub(crate) struct Downloads {
    pub config: Option<DownloadConfig>,
    pub list: DownloadList,
    waiting: BTreeMap<DownloadHandle, Waiting>,
    items: BTreeMap<DownloadHandle, Item>,
}

fn same(browser: &Weak<Slot>, slot: &Slot) -> bool {
    std::ptr::eq(browser.as_ptr(), slot)
}

impl Downloads {
    pub fn before(
        &mut self,
        browser: Weak<Slot>,
        offer: Offer<'_>,
        proceed: Box<dyn Proceed>,
        exists: impl Fn(&Path) -> bool,
    ) -> Vec<Act> {
        let handle = offer.handle;
        let blocked = |browser: Weak<Slot>, proceed| {
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
            return blocked(browser, proceed);
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
                save_as: false,
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
            return blocked(browser, proceed);
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
                self.waiting.insert(handle, waiting);
                vec![Act::Emit(
                    browser,
                    DownloadEvent::AskPath {
                        handle,
                        url: offer.url.into(),
                        suggested,
                        reason,
                    },
                )]
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
            let url = waiting.delivery.url.clone();
            return vec![
                Act::Cancel(waiting.proceed),
                Act::Emit(browser, DownloadEvent::Blocked { handle, url }),
            ];
        };
        match warning {
            Some(warning) => {
                waiting.stage = Stage::Held { id, path };
                self.waiting.insert(handle, waiting);
                vec![Act::Emit(
                    browser,
                    DownloadEvent::Held {
                        handle,
                        id,
                        warning,
                    },
                )]
            }
            None => vec![
                Act::Proceed(waiting.proceed, path.clone()),
                Act::Emit(browser, DownloadEvent::Started { handle, id, path }),
            ],
        }
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
                let Some(name) = name.filter(|_| target.is_absolute()) else {
                    self.waiting.insert(handle, waiting);
                    return Err(DownloadError::InvalidPath(target));
                };
                if let Some(folder) = target.parent() {
                    // The next save dialog starts where this one was answered.
                    self.list.downloads_folder_changed(folder);
                }
                let https_first = self
                    .config
                    .as_ref()
                    .is_some_and(|config| config.https_first_mode);
                // Choosing the path approves the file type, as in Chromium, but the new name can
                // still make an insecure delivery one Chromium holds or blocks.
                match waiting.delivery.verdict(&name, https_first) {
                    InsecureVerdict::SilentBlock => {
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
                Ok(vec![
                    Act::Proceed(waiting.proceed, decided.clone()),
                    Act::Emit(
                        waiting.browser,
                        DownloadEvent::Started {
                            handle,
                            id,
                            path: decided,
                        },
                    ),
                ])
            }
        }
    }

    /// Discards a waiting download, or cancels one in progress.
    pub fn cancel(&mut self, handle: DownloadHandle) -> Result<Vec<Act>, DownloadError> {
        if let Some(waiting) = self.waiting.remove(&handle) {
            let id = match waiting.stage {
                Stage::Held { id, .. } => self.list.discard(id).ok().map(|_| id),
                Stage::Ask { .. } => None,
            };
            return Ok(vec![
                Act::Cancel(waiting.proceed),
                Act::Emit(waiting.browser, DownloadEvent::Cancelled { handle, id }),
            ]);
        }
        let (id, control) = self.live(handle)?;
        self.list.cancel(id)?;
        Ok(vec![Act::CancelItem(control)])
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
        let item = self
            .items
            .get(&handle)
            .ok_or(DownloadError::Unknown(handle))?;
        Ok((id, item.control.clone()))
    }

    /// `OnDownloadUpdated`.
    pub fn updated(
        &mut self,
        browser: Weak<Slot>,
        handle: DownloadHandle,
        report: Option<Snapshot<'_>>,
        control: Rc<dyn Control>,
    ) -> Vec<Act> {
        let Some(snapshot) = report else {
            return Vec::new();
        };
        let state = snapshot.state;
        let ended = matches!(state, SnapshotState::Complete | SnapshotState::Cancelled);
        if ended {
            self.items.remove(&handle);
        } else {
            self.items.insert(
                handle,
                Item {
                    control,
                    browser: browser.clone(),
                },
            );
        }
        let mut acts = Vec::new();
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
        acts
    }

    /// `browser` closed: cancel what it started, as CEF does with a destroyed browser's items.
    pub fn browser_closed(&mut self, browser: &Slot) -> Vec<Act> {
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
            acts.push(Act::CancelItem(item.control));
            acts.push(Act::Emit(
                item.browser,
                DownloadEvent::Cancelled { handle, id },
            ));
        }
        acts
    }

    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.waiting.is_empty() && self.items.is_empty()
    }
}

impl crate::Engine {
    /// The folder and prompt settings downloads follow. `None` refuses every download.
    pub fn set_download_config(&self, config: Option<DownloadConfig>) {
        self.services().downloads.borrow_mut().config = config;
    }

    /// The sidebar's list. Change it through the engine so CEF and the list agree.
    pub fn downloads(&self) -> std::cell::Ref<'_, DownloadList> {
        std::cell::Ref::map(self.services().downloads.borrow(), |downloads| {
            &downloads.list
        })
    }

    /// Installs the list loaded at startup (`flecha_downloads::persist::load`). Call it before
    /// the first download.
    pub fn restore_downloads(&self, list: DownloadList) {
        self.services().downloads.borrow_mut().list = list;
    }

    /// Drops a finished, cancelled or interrupted row. The file stays on disk.
    pub fn remove_download(&self, id: DownloadId) -> Result<(), DownloadError> {
        let mut downloads = self.services().downloads.borrow_mut();
        downloads.list.remove(id).map(drop).map_err(Into::into)
    }

    /// `clearDownloads`: drops every row started after `since` that isn't progressing.
    pub fn clear_downloads(&self, since: EpochMillis) -> Vec<DownloadId> {
        self.services().downloads.borrow_mut().list.clear(since)
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

    pub fn next_download_event(&mut self) -> Option<DownloadMessage> {
        self.adopt_popups();
        let (tab, event) = self.services().download_events.pop()?;
        Some(DownloadMessage { tab, event })
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
                handle: DownloadHandle(item.id()),
                url: &url,
                original_url: &original_url,
                content_disposition: disposition.as_deref(),
                suggested_name: suggested.as_deref(),
                initiator: initiator.as_deref(),
                started: chrono::Local::now().naive_local(),
                started_at: epoch_millis(),
            };
            let services = services::of(&self.slot);
            let acts = services.downloads.borrow_mut().before(
                Rc::downgrade(&self.slot),
                offer,
                Box::new(Before(callback.clone())),
                Path::exists,
            );
            run(&services, acts);
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
            let services = services::of(&self.slot);
            let report = state.map(|state| Snapshot {
                state,
                received: item.received_bytes(),
                total: item.total_bytes(),
                full_path: &full_path,
            });
            let acts = services.downloads.borrow_mut().updated(
                Rc::downgrade(&self.slot),
                DownloadHandle(item.id()),
                report,
                Rc::new(callback.clone()),
            );
            run(&services, acts);
        }
    }
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
        settle(downloads.before(browser, offer, proceed, |_| false))
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
            DownloadHandle(1),
            report(SnapshotState::InProgress, 10, 100, &intermediate),
            item.clone(),
        ));
        assert_eq!(downloads.list.get(1).unwrap().path(), path);
        assert!(downloads.list.reserves(&path, Platform::current()));
        settle(downloads.updated(
            Weak::new(),
            DownloadHandle(1),
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
                DownloadHandle(handle),
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
            DownloadHandle(9),
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
}
