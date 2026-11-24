//! The responsive view (chromium/shell/features/responsive.ts): the shown tab again as a
//! phone-sized mirror beside it. The mirror emulates a phone and follows the desktop side's
//! navigation, never the other way: sites often send phones to an `m.` domain, which must not
//! drag the desktop side along.

use flecha_core::{Device, SplitMode, TabId, WindowId};

use crate::Controller;

impl Controller {
    /// The mirrors whose desktop side is `source`, in every responsive split of every window,
    /// including Spaces the window does not show.
    fn mirrors_of(&self, source: &TabId) -> Vec<TabId> {
        let windows: Vec<WindowId> = self
            .model
            .regular_windows()
            .iter()
            .chain(self.model.incognito_windows())
            .map(|window| window.id.clone())
            .collect();
        windows
            .iter()
            .filter_map(|id| self.model.state(id)?.window(id))
            .flat_map(|current| current.spaces.values())
            .filter_map(|space| {
                let [desktop, mirror] = space.split.as_ref()?;
                (space.split_mode == Some(SplitMode::Responsive) && desktop == source)
                    .then(|| mirror.clone())
            })
            .collect()
    }

    /// Sends `source`'s mirrors to the address it just reached. Returns whether a mirror without
    /// a browser took the address in saved state, which then needs saving.
    pub(crate) fn follow_responsive(&mut self, source: &TabId) -> bool {
        let Some(url) = self.model.tab(source).map(|tab| tab.url.clone()) else {
            return false;
        };
        let mut save = false;
        for mirror in self.mirrors_of(source) {
            let Some((tab, durable)) = self.model.tab_mut(&mirror) else {
                continue;
            };
            if tab.url == url || self.engine.navigate(&mirror, &url) {
                continue;
            }
            tab.title.clone_from(&url);
            tab.url = url.clone();
            save |= durable;
        }
        save
    }

    /// A mirror's browser, created blank, emulates a phone and then loads the mirror's page.
    pub(crate) fn emulate_mirror(&mut self, id: &TabId) {
        let Some(url) = self
            .model
            .tab(id)
            .filter(|tab| tab.device == Some(Device::Mobile))
            .map(|tab| tab.url.clone())
        else {
            return;
        };
        if let Err(error) = self.engine.emulate_phone(id, &url) {
            let session = self
                .model
                .partition(id)
                .and_then(|partition| partition.incognito_session());
            self.status.set(
                session,
                format!("Responsive view could not emulate a phone. {error:#}"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;

    #[test]
    fn the_responsive_view_opens_a_mirror_that_follows_and_closes_with_it() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let desktop = controller
            .model
            .open(&window, "https://example.test/".into(), true)
            .unwrap();

        controller.act(&window, Action::ToggleResponsive).unwrap();
        let current = controller.model.core.window(&window).unwrap();
        let space = &current.spaces[&current.active_space_id];
        let [left, mirror] = space.split.clone().unwrap();
        assert_eq!(left, desktop);
        assert_eq!(space.split_mode, Some(SplitMode::Responsive));
        let phone = controller.model.tab(&mirror).unwrap();
        assert_eq!(phone.device, Some(Device::Mobile));
        assert_eq!(phone.url, "https://example.test/");

        // The desktop side's navigation reaches the mirror, which has no browser here.
        controller
            .model
            .tab_mut(&desktop)
            .unwrap()
            .0
            .url
            .clone_from(&"https://example.test/next".to_owned());
        assert!(controller.follow_responsive(&desktop));
        assert_eq!(
            controller.model.tab(&mirror).unwrap().url,
            "https://example.test/next"
        );
        // The mirror never drags the desktop side along.
        assert!(!controller.follow_responsive(&mirror));
        assert_eq!(
            controller.model.tab(&desktop).unwrap().url,
            "https://example.test/next"
        );

        // It follows while its Space is out of sight too.
        let home = controller
            .model
            .core
            .window(&window)
            .unwrap()
            .active_space_id
            .clone();
        controller.act(&window, Action::NewSpace).unwrap();
        controller
            .model
            .tab_mut(&desktop)
            .unwrap()
            .0
            .url
            .clone_from(&"https://example.test/hidden".to_owned());
        assert!(controller.follow_responsive(&desktop));
        assert_eq!(
            controller.model.tab(&mirror).unwrap().url,
            "https://example.test/hidden"
        );
        controller.act(&window, Action::Switch(home)).unwrap();

        controller.act(&window, Action::ToggleResponsive).unwrap();
        let current = controller.model.core.window(&window).unwrap();
        let space = &current.spaces[&current.active_space_id];
        assert_eq!(space.split, None);
        assert_eq!(space.active_tab_id.as_ref(), Some(&desktop));
        assert!(controller.model.tab(&mirror).is_none());
    }

    #[test]
    fn the_divider_ratio_is_saved_within_the_shell_range() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        controller
            .model
            .open(&window, "https://left.test/".into(), true)
            .unwrap();
        controller
            .act(&window, Action::SplitNew("https://right.test/".into()))
            .unwrap();
        let ratio = |controller: &Controller| {
            let current = controller.model.core.window(&window).unwrap();
            current.spaces[&current.active_space_id].split_ratio
        };
        controller
            .act(&window, Action::SetSplitRatio(0.65))
            .unwrap();
        assert_eq!(ratio(&controller), Some(0.65));
        controller
            .act(&window, Action::SetSplitRatio(0.95))
            .unwrap();
        assert_eq!(
            ratio(&controller),
            Some(*flecha_core::SPLIT_RATIO_RANGE.end())
        );
        drop(controller);

        let reopened = Controller::open_test_profile(profile.path()).unwrap();
        let current = reopened.model.core.window(&window).unwrap();
        assert_eq!(
            current.spaces[&current.active_space_id].split_ratio,
            Some(*flecha_core::SPLIT_RATIO_RANGE.end())
        );
    }
}
