//! Incognito windows, as the shipping shell keeps them (`chromium/shell/incognito.ts`).
//!
//! The incognito windows of a session share one [`BrowserState`] of their own: one Space of
//! tabs, no pinned tabs, favorites, Space switcher or archive. It lives in [`crate::model::Model`]
//! next to the regular state and is never handed to storage, so nothing about these windows
//! reaches the session snapshot. Their pages browse in one in-memory CEF request context
//! ([`Partition::Incognito`]): cookies, cache, local storage and permission decisions stay in
//! memory. History never records them (`History::record` takes persistent partitions only), and
//! the command bar asks no search provider for suggestions while one is in front.
//!
//! Preferences are the regular state's (`incognito.ts` `followedPreferences`): read
//! `Model::core.settings`, never this state's own settings.
//!
//! Closing the last incognito window ends the session: its state is dropped and the engine
//! lets go of its request context ([`flecha_cef::Engine::end_incognito`]).

use flecha_cef::{IncognitoSession, Owner, Partition};
use flecha_core::{BrowserState, SpaceId, WindowId};

use crate::sidebar::SidebarAction;
use crate::site_controls::SiteChange;
use crate::{Action, Controller};

/// The one Space of an incognito window (`INCOGNITO_SPACE_ID` and `incognitoState`).
const SPACE_ID: &str = "incognito";
const SPACE_NAME: &str = "Incognito";
const SPACE_EMOJI: &str = "🕶️";
const SPACE_COLOR: &str = "#ddd5f3";

/// An incognito window never looks like a regular one (patch 0032 draws its pages dark): its
/// window and sidebar take the incognito Space's violet, darkened.
pub(crate) const BACKGROUND: u32 = 0x1c1727;
pub(crate) const SIDEBAR: u32 = 0x2a2338;
pub(crate) const LABEL: u32 = 0xddd5f3;

pub(crate) struct Incognito {
    pub(crate) session: IncognitoSession,
    pub(crate) state: BrowserState,
}

impl Incognito {
    pub(crate) fn new(session: IncognitoSession, window: WindowId) -> Self {
        let space = SpaceId::new(SPACE_ID);
        let mut state = BrowserState::new(space.clone(), window);
        state.update_space(
            &space,
            Some(SPACE_NAME),
            Some(SPACE_EMOJI),
            Some(SPACE_COLOR),
            None,
        );
        Self { session, state }
    }

    pub(crate) fn partition(&self) -> Partition {
        Partition::Incognito(self.session)
    }
}

impl Controller {
    /// Closes an incognito window and its browsers. The last one ends the session: the pages
    /// it opened close too, and the engine lets go of its request context.
    pub(crate) fn close_incognito_window(&mut self, window: &WindowId) {
        let shown = self
            .engine
            .ids()
            .filter(|id| self.engine.owner(id) == Some(&Owner::Tab(window.clone())))
            .cloned()
            .collect::<Vec<_>>();
        let closed = self.model.close_incognito_window(window);
        for id in shown.iter().chain(&closed.removed) {
            self.engine.close(id);
        }
        if let Some(session) = closed.ended {
            self.engine.end_incognito(session);
        }
        self.update_visibility();
    }
}

/// Why an incognito window refuses `action`, if it does. It has one Space of tabs that goes
/// with the window (`shell.css`, `background.ts` `spaceMenu`), and it writes nothing that would
/// name a site to the regular profile (`siteControls.ts` `setSiteAdBlock`).
pub(crate) fn refusal(action: &Action) -> Option<&'static str> {
    match action {
        Action::Switch(_) | Action::NewSpace | Action::Isolate => {
            Some("An incognito window has one Space")
        }
        Action::RestoreArchive(_) | Action::ClearArchive => {
            Some("An incognito window has no archive")
        }
        Action::Sidebar(
            SidebarAction::Pin(..)
            | SidebarAction::Favorite(_)
            | SidebarAction::FolderName(..)
            | SidebarAction::ToggleFolder(..)
            | SidebarAction::RemoveFolder(_)
            | SidebarAction::TogglePinned(_),
        ) => Some("An incognito window has no pinned tabs or favorites"),
        Action::Site(_, SiteChange::Shield(_)) => Some("Change it in a regular window"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use flecha_cef::Disposition;
    use flecha_core::TabId;

    use super::*;
    use crate::model::now;
    use crate::popup::{Place, place};
    use crate::site_controls::Site;

    const REGULAR: &str = "https://regular.example/kept-4f1c";
    const PRIVATE: &str = "https://private.example/never-saved-9b2e";

    /// Every file under `dir` that contains `needle`.
    fn files_containing(dir: &Path, needle: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_containing(&path, needle));
            } else if std::fs::read(&path)
                .unwrap()
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
            {
                found.push(path);
            }
        }
        found
    }

    fn incognito_window(controller: &mut Controller, regular: &WindowId) -> WindowId {
        controller.act(regular, Action::NewIncognitoWindow).unwrap();
        controller.model.incognito_windows()[0].id.clone()
    }

    #[test]
    fn incognito_tabs_never_reach_the_profile_on_disk() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let regular = controller.model.core.focused_window().id.clone();
        controller
            .act(&regular, Action::Open(REGULAR.into()))
            .unwrap();
        let private = incognito_window(&mut controller, &regular);
        controller
            .act(&private, Action::Open(PRIVATE.into()))
            .unwrap();
        let tab = controller.model.active(&private).unwrap().clone();

        // A page's own address and title changes, as CEF reports them.
        let partition = controller.model.partition(&tab).unwrap();
        assert!(partition.is_incognito());
        let (page, durable) = controller.model.tab_mut(&tab).unwrap();
        assert!(!durable, "an incognito tab counts as saved state");
        page.title = format!("{PRIVATE} title");
        for action in [
            Action::SplitNew(format!("{PRIVATE}/split")),
            Action::ExitSplit,
            Action::Peek(format!("{PRIVATE}/peek")),
            Action::Promote,
            Action::CloseTab(tab.clone()),
            Action::Reopen,
        ] {
            controller.act(&private, action).unwrap();
        }
        // History takes no visit from an incognito page, even one marked durable.
        assert!(
            controller
                .history
                .record(true, Some(&partition), PRIVATE, "", now())
                .unwrap()
                .is_none()
        );
        controller.save().unwrap();

        assert!(
            !files_containing(profile.path(), REGULAR).is_empty(),
            "the scan must find what a regular window saved"
        );
        for marker in ["private.example", private.as_str()] {
            let found = files_containing(profile.path(), marker);
            assert!(found.is_empty(), "{marker} reached {found:?}");
        }

        // A restart restores the regular window alone.
        drop(controller);
        let restarted = Controller::open_test_profile(profile.path()).unwrap();
        assert!(restarted.model.incognito.is_none());
        assert_eq!(restarted.model.core.windows().len(), 1);
        assert!(
            restarted
                .model
                .core
                .open_tabs()
                .values()
                .all(|tab| !tab.url.contains("private.example"))
        );
    }

    #[test]
    fn an_incognito_window_keeps_one_space_without_pins_or_favorites() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let regular = controller.model.core.focused_window().id.clone();
        let private = incognito_window(&mut controller, &regular);
        controller
            .act(&private, Action::Open(PRIVATE.into()))
            .unwrap();
        let tab = controller.model.active(&private).unwrap().clone();
        let partition = controller.model.partition(&tab).unwrap();
        let site = Site::from_url(tab.clone(), partition, PRIVATE).unwrap();
        let before = controller.model.core.clone();
        let space = before.focused_window().active_space_id.clone();
        for action in [
            Action::NewSpace,
            Action::Switch(space),
            Action::Isolate,
            Action::ClearArchive,
            Action::Sidebar(SidebarAction::Pin(tab.clone(), None)),
            Action::Sidebar(SidebarAction::Favorite(tab.clone())),
            Action::Sidebar(SidebarAction::FolderName(None, "Folder".into())),
            Action::Site(site.clone(), SiteChange::Shield(false)),
        ] {
            let refused = refusal(&action);
            let error = controller.act(&private, action).unwrap_err();
            assert_eq!(Some(error.to_string().as_str()), refused);
        }
        assert_eq!(
            controller.model.core, before,
            "a refusal changed the regular state"
        );
        let incognito = &controller.model.incognito.as_ref().unwrap().state;
        assert_eq!(incognito.spaces().len(), 1);
        assert_eq!(incognito.spaces()[0].name, SPACE_NAME);
        assert!(incognito.favorites().is_empty());
        assert!(incognito.spaces()[0].pinned.is_empty());
        assert!(controller.model.partition(&tab).unwrap().is_incognito());
    }

    #[test]
    fn pages_an_incognito_page_opens_stay_incognito() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let regular = controller.model.core.focused_window().id.clone();
        let private = incognito_window(&mut controller, &regular);
        let session = Partition::Incognito(controller.model.incognito.as_ref().unwrap().session);

        // A Peek with no page behind it takes the window's context, not a Space's.
        controller
            .act(&private, Action::Peek(PRIVATE.into()))
            .unwrap();
        let peek = controller.model.peeks[&private].id.clone();
        assert_eq!(controller.model.partition(&peek), Some(session.clone()));
        controller.act(&private, Action::ClosePeek).unwrap();

        controller
            .act(&private, Action::Open(PRIVATE.into()))
            .unwrap();
        let tab = controller.model.active(&private).unwrap().clone();
        let owner = Owner::Tab(private.clone());
        assert_eq!(
            place(&controller.model, &owner, &tab, Disposition::ForegroundTab),
            Place::Tab {
                window: private.clone(),
                after: Some(tab.clone()),
                activate: true,
            }
        );

        // A Flechinha keeps the context, sends its links to an incognito window and goes back
        // to one.
        controller.act(&private, Action::DetachTab(tab)).unwrap();
        let (mini, page) = controller
            .model
            .flechinhas
            .iter()
            .map(|(window, page)| (window.clone(), page.id.clone()))
            .next()
            .unwrap();
        assert_eq!(controller.model.partition(&page), Some(session.clone()));
        assert!(controller.model.is_incognito(&mini));
        controller.model.focus_window(&regular);
        for owner in [Owner::Flechinha(mini.clone())] {
            assert_eq!(
                place(&controller.model, &owner, &page, Disposition::BackgroundTab),
                Place::Tab {
                    window: private.clone(),
                    after: None,
                    activate: false,
                }
            );
        }
        controller.act(&mini, Action::Return).unwrap();
        assert_eq!(controller.model.active(&private), Some(&page));
        assert!(controller.model.core.tab(&page).is_none());

        // A link that asks for a new window gets an incognito one.
        let opened = controller
            .model
            .open_window_page(TabId::new("opened"), PRIVATE.into(), &session)
            .unwrap();
        assert!(controller.model.is_incognito(&opened));
        assert_eq!(controller.model.core.windows().len(), 1);
    }

    #[test]
    fn closing_the_last_incognito_window_ends_its_session() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let regular = controller.model.core.focused_window().id.clone();
        let first = incognito_window(&mut controller, &regular);
        controller
            .act(&first, Action::Open(PRIVATE.into()))
            .unwrap();
        let session = controller.model.incognito.as_ref().unwrap().session;
        controller
            .act(&regular, Action::NewIncognitoWindow)
            .unwrap();
        assert_eq!(controller.model.incognito_windows().len(), 2);
        let second = controller.model.incognito_windows()[0].id.clone();
        assert_ne!(first, second);
        controller
            .act(&second, Action::Open(format!("{PRIVATE}/second")))
            .unwrap();
        let tab = controller.model.active(&second).unwrap().clone();
        controller.act(&second, Action::DetachTab(tab)).unwrap();
        let mini = controller.model.flechinhas.keys().next().unwrap().clone();

        controller.act(&first, Action::CloseWindow).unwrap();
        assert_eq!(
            controller.model.incognito.as_ref().map(|i| i.session),
            Some(session),
            "the session lasts while an incognito window is open"
        );
        controller.act(&second, Action::CloseWindow).unwrap();
        assert!(controller.model.incognito.is_none());
        assert!(
            !controller.model.flechinhas.contains_key(&mini),
            "the session's Flechinha outlived it"
        );
        assert!(
            !controller.quitting,
            "closing incognito windows quit Flecha"
        );
        assert_eq!(controller.model.core.windows().len(), 1);
        assert!(
            controller
                .model
                .open_window_page(
                    TabId::new("late"),
                    PRIVATE.into(),
                    &Partition::Incognito(session)
                )
                .is_err(),
            "a page of an ended session reopened"
        );

        controller
            .act(&regular, Action::NewIncognitoWindow)
            .unwrap();
        let next = controller.model.incognito.as_ref().unwrap();
        assert_ne!(
            next.session, session,
            "a new session reused the old storage"
        );
        assert!(next.state.open_tabs().is_empty());
    }
}
