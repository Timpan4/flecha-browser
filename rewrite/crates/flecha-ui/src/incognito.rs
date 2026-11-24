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
