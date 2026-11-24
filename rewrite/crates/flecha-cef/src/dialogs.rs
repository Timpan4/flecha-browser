use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use cef::{rc::Rc as _, *};
use flecha_core::TabId;

use crate::Engine;
use crate::callbacks::{Slot, Stage};

#[derive(Clone)]
pub struct DialogId(Rc<()>);

impl PartialEq for DialogId {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub struct DialogRequest {
    pub id: DialogId,
    pub kind: DialogKind,
}

#[derive(Clone)]
pub enum DialogKind {
    Alert {
        origin: String,
        message: String,
    },
    Confirm {
        origin: String,
        message: String,
    },
    Prompt {
        origin: String,
        message: String,
        default: String,
    },
    BeforeUnload {
        reload: bool,
    },
    Auth {
        host: String,
        port: i32,
        realm: String,
        proxy: bool,
        https: bool,
    },
    File {
        mode: FileMode,
        default_path: PathBuf,
    },
}

#[derive(Clone, Copy)]
pub enum FileMode {
    Open,
    OpenMultiple,
    Folder,
    Save,
}

pub enum DialogResponse {
    Cancel,
    Accept(String),
    Credentials { username: String, password: String },
    Paths(Vec<PathBuf>),
}

enum Answer {
    Js(JsdialogCallback),
    Auth(AuthCallback),
    File(FileDialogCallback),
}

impl Answer {
    fn finish(self, response: DialogResponse) {
        match (self, response) {
            (Self::Js(callback), DialogResponse::Accept(text)) => {
                callback.cont(1, Some(&CefString::from(text.as_str())));
            }
            (Self::Js(callback), _) => callback.cont(0, None),
            (Self::Auth(callback), DialogResponse::Credentials { username, password }) => {
                callback.cont(
                    Some(&CefString::from(username.as_str())),
                    Some(&CefString::from(password.as_str())),
                );
            }
            (Self::Auth(callback), _) => callback.cancel(),
            (Self::File(callback), DialogResponse::Paths(paths)) if !paths.is_empty() => {
                let mut list = CefStringList::default();
                if paths.iter().all(|path| {
                    path.is_absolute() && path.to_str().is_some_and(|path| list.append(path))
                }) {
                    callback.cont(Some(&mut list));
                } else {
                    callback.cancel();
                }
            }
            (Self::File(callback), _) => callback.cancel(),
        }
    }
}

struct Pending {
    request: DialogRequest,
    answer: Answer,
    picker_open: bool,
}

#[derive(Default)]
pub(crate) struct Dialogs(RefCell<VecDeque<Pending>>);

impl Dialogs {
    fn push(&self, kind: DialogKind, answer: Answer) {
        self.0.borrow_mut().push_back(Pending {
            request: DialogRequest {
                id: DialogId(Rc::new(())),
                kind,
            },
            answer,
            picker_open: false,
        });
    }

    fn take(&self, id: &DialogId) -> Option<Answer> {
        let mut queue = self.0.borrow_mut();
        let index = queue.iter().position(|item| &item.request.id == id)?;
        Some(queue.remove(index)?.answer)
    }

    pub(crate) fn cancel(&self) -> bool {
        let pending = self.0.take();
        let changed = !pending.is_empty();
        for item in pending {
            item.answer.finish(DialogResponse::Cancel);
        }
        changed
    }

    fn reset_js(&self) -> bool {
        let mut queue = self.0.borrow_mut();
        let mut cancelled = Vec::new();
        let mut remaining = VecDeque::new();
        for item in queue.drain(..) {
            if matches!(item.answer, Answer::Js(_)) {
                cancelled.push(item.answer);
            } else {
                remaining.push_back(item);
            }
        }
        *queue = remaining;
        drop(queue);
        let changed = !cancelled.is_empty();
        for answer in cancelled {
            answer.finish(DialogResponse::Cancel);
        }
        changed
    }
}

fn enqueue(slot: &Slot, kind: DialogKind, answer: Answer) {
    slot.dialogs.push(kind, answer);
    slot.emit(crate::Event::DialogChanged);
}

// The request handler serves both the IO and UI threads. CEF posts only the browser id
// and refcounted auth callback across threads. Slot and dialog state stay on the UI thread.
thread_local! {
    static BROWSERS: RefCell<BTreeMap<i32, Weak<Slot>>> = const { RefCell::new(BTreeMap::new()) };
}

pub(crate) fn register(slot: &Rc<Slot>, identifier: i32) {
    BROWSERS.with_borrow_mut(|browsers| {
        browsers.insert(identifier, Rc::downgrade(slot));
    });
}

pub(crate) fn unregister(slot: &Slot) {
    if let Some(id) = slot.identifier.get() {
        BROWSERS.with_borrow_mut(|browsers| browsers.remove(&id));
    }
}

fn live_slot(identifier: i32) -> Option<Rc<Slot>> {
    BROWSERS
        .with_borrow(|browsers| browsers.get(&identifier)?.upgrade())
        .filter(|slot| !slot.retired.get() && slot.stage.get() == Stage::Live)
}

wrap_task! {
    struct AuthTask { browser: i32, committed: u64, kind: DialogKind, callback: AuthCallback }
    impl Task {
        fn execute(&self) {
            if let Some(slot) = live_slot(self.browser).filter(|slot| {
                slot.committed_navigation.load(std::sync::atomic::Ordering::SeqCst) == self.committed
            }) {
                enqueue(&slot, self.kind.clone(), Answer::Auth(self.callback.clone()));
            } else {
                self.callback.cancel();
            }
        }
    }
}

pub(crate) struct AuthChallenge {
    pub host: String,
    pub port: i32,
    pub realm: String,
    pub proxy: bool,
    pub https: bool,
    pub committed: u64,
}

pub(crate) fn authenticate(
    browser: &Browser,
    challenge: AuthChallenge,
    callback: &AuthCallback,
) -> i32 {
    let AuthChallenge {
        host,
        port,
        realm,
        proxy,
        https,
        committed,
    } = challenge;
    let kind = DialogKind::Auth {
        host,
        port,
        realm,
        proxy,
        https,
    };
    let mut task = AuthTask::new(browser.identifier(), committed, kind, callback.clone());
    post_task(ThreadId::UI, Some(&mut task))
}

pub(crate) fn certificate_error(browser: &Browser, url: &CefString) {
    if let Some(slot) = live_slot(browser.identifier()) {
        slot.certificate_error.replace(Some(url.to_string()));
        slot.emit(crate::Event::SecurityChanged);
    }
}

pub(crate) fn new_navigation(browser: &Browser) {
    if let Some(slot) = live_slot(browser.identifier())
        && slot.certificate_error.take().is_some()
    {
        slot.emit(crate::Event::SecurityChanged);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectionSecurity {
    Unknown,
    VerifiedHttps,
    CertificateError,
    InsecureContent,
}

fn ssl_security(ssl: &Sslstatus) -> Option<ConnectionSecurity> {
    use cef::sys::{_cef_sslstatus_t, cef_cert_status_t, cef_is_cert_status_error};

    type GetFlags = unsafe extern "C" fn(*mut _cef_sslstatus_t) -> u32;
    type CheckCertFlags = unsafe extern "C" fn(u32) -> i32;
    let raw = ssl.get_raw();
    // CEF returns bitmasks. The generated bindings use repr(u32) enums whose Rust
    // discriminants cover single flags only. Call with the same C ABI and integer
    // representation so combinations never become invalid Rust enum values.
    let (cert_error, content_flags) = unsafe {
        let cert: GetFlags = std::mem::transmute((*raw).get_cert_status?);
        let content: GetFlags = std::mem::transmute((*raw).get_content_status?);
        let check: CheckCertFlags = std::mem::transmute(
            cef_is_cert_status_error as unsafe extern "C" fn(cef_cert_status_t) -> i32,
        );
        (check(cert(raw)), content(raw))
    };
    Some(if cert_error != 0 {
        ConnectionSecurity::CertificateError
    } else if ssl.is_secure_connection() == 0 {
        ConnectionSecurity::Unknown
    } else if content_flags != SslContentStatus::NORMAL_CONTENT.get_raw() {
        ConnectionSecurity::InsecureContent
    } else {
        ConnectionSecurity::VerifiedHttps
    })
}

impl Engine {
    pub fn connection_security(&self, tab: &TabId, address: &str) -> ConnectionSecurity {
        let Some(session) = self.browsers.get(tab) else {
            return ConnectionSecurity::Unknown;
        };
        if session.slot.certificate_error.borrow().as_deref() == Some(address) {
            return ConnectionSecurity::CertificateError;
        }
        let Some(entry) = self
            .host(tab)
            .and_then(|host| host.visible_navigation_entry())
        else {
            return ConnectionSecurity::Unknown;
        };
        if !address.starts_with("https:") || CefString::from(&entry.url()).to_string() != address {
            return ConnectionSecurity::Unknown;
        }
        let Some(ssl) = entry.sslstatus() else {
            return ConnectionSecurity::Unknown;
        };
        ssl_security(&ssl).unwrap_or(ConnectionSecurity::Unknown)
    }

    pub fn pending_dialog(&self, tab: &TabId) -> Option<DialogRequest> {
        let slot = &self.browsers.get(tab)?.slot;
        let queue = slot.dialogs.0.borrow();
        Some(queue.front()?.request.clone())
    }

    /// Claim the current file dialog once, even when the browser changes windows.
    pub fn claim_file_dialog(&self, tab: &TabId, id: &DialogId) -> bool {
        let Some(session) = self.browsers.get(tab) else {
            return false;
        };
        let mut queue = session.slot.dialogs.0.borrow_mut();
        let Some(item) = queue
            .front_mut()
            .filter(|item| &item.request.id == id && matches!(item.answer, Answer::File(_)))
        else {
            return false;
        };
        !std::mem::replace(&mut item.picker_open, true)
    }

    pub fn resolve_dialog(&self, id: &DialogId, response: DialogResponse) {
        let answer = self
            .browsers
            .values()
            .find_map(|session| session.slot.dialogs.take(id));
        if let Some(answer) = answer {
            answer.finish(response);
        }
    }

    /// Ask CEF to close a live tab after any beforeunload confirmation.
    pub fn request_tab_close(&self, tab: &TabId) -> bool {
        let Some(session) = self.browsers.get(tab) else {
            return false;
        };
        let queue = session.slot.dialogs.0.borrow();
        if !queue.is_empty() {
            return queue
                .iter()
                .any(|dialog| matches!(dialog.request.kind, DialogKind::BeforeUnload { .. }));
        }
        drop(queue);
        let Some(host) = self.host(tab) else {
            return false;
        };
        host.close_browser(0);
        true
    }
}

wrap_jsdialog_handler! {
    pub(crate) struct JsDialogs { slot: Rc<Slot> }
    impl JsdialogHandler {
        fn on_jsdialog(
            &self, _browser: Option<&mut Browser>, origin: Option<&CefString>,
            kind: JsdialogType, message: Option<&CefString>, default: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>, suppress: Option<&mut i32>,
        ) -> i32 {
            if self.slot.retired.get() {
                if let Some(suppress) = suppress { *suppress = 1; }
                return 0;
            }
            let Some(callback) = callback else { return 0 };
            let origin = text(origin);
            let message = text(message);
            let kind = match kind {
                JsdialogType::ALERT => DialogKind::Alert { origin, message },
                JsdialogType::CONFIRM => DialogKind::Confirm { origin, message },
                JsdialogType::PROMPT => DialogKind::Prompt { origin, message, default: text(default) },
                _ => return 0,
            };
            enqueue(&self.slot, kind, Answer::Js(callback.clone()));
            1
        }

        fn on_before_unload_dialog(
            &self, _browser: Option<&mut Browser>, _message: Option<&CefString>,
            reload: i32, callback: Option<&mut JsdialogCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 0 };
            if self.slot.retired.get() {
                callback.cont(1, None);
            } else {
                enqueue(&self.slot, DialogKind::BeforeUnload { reload: reload != 0 }, Answer::Js(callback.clone()));
            }
            1
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            if self.slot.dialogs.reset_js() {
                self.slot.emit(crate::Event::DialogChanged);
            }
        }
    }
}

wrap_dialog_handler! {
    pub(crate) struct FileDialogs { slot: Rc<Slot> }
    impl DialogHandler {
        fn on_file_dialog(
            &self, _browser: Option<&mut Browser>, mode: FileDialogMode, _title: Option<&CefString>,
            default_path: Option<&CefString>, _filters: Option<&mut CefStringList>,
            _extensions: Option<&mut CefStringList>, _descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 0 };
            if self.slot.retired.get() {
                callback.cancel();
                return 1;
            }
            let mode = match mode {
                FileDialogMode::OPEN => FileMode::Open,
                FileDialogMode::OPEN_MULTIPLE => FileMode::OpenMultiple,
                FileDialogMode::OPEN_FOLDER => FileMode::Folder,
                FileDialogMode::SAVE => FileMode::Save,
                _ => return 0,
            };
            enqueue(
                &self.slot,
                DialogKind::File { mode, default_path: text(default_path).into() },
                Answer::File(callback.clone()),
            );
            1
        }
    }
}

pub(crate) fn text(value: Option<&CefString>) -> String {
    value.map(ToString::to_string).unwrap_or_default()
}
