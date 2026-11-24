//! What a Space with no tab shown offers (`#empty-state`, ui/index.html:167-192 and
//! ui/app.ts:247-304): a greeting, the search field that opens the command bar, the sites you
//! go back to most, your dev servers, and a few shortcuts.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use chrono::Timelike as _;
use flecha_core::{Space, WindowId, shortcuts};
use flecha_history::HistoryEntry;
use flecha_services::local_servers::{LocalServer, LocalServerWatch};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::command_bar::Command;
use crate::icons::icon;
use crate::shortcuts::platform;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Shell, hairline, kbd, ring, site};
use crate::{Action, Controller, dispatch};

/// The dev server scan. It starts on the first UI tick, so a controller that never runs the
/// UI, such as a unit test's, never reads the host's sockets or asks its servers for pages.
#[derive(Default)]
pub(crate) enum LocalWatch {
    #[default]
    Unstarted,
    Running(LocalServerWatch),
    Unavailable,
}

impl LocalWatch {
    pub(crate) fn get(&self) -> Option<&LocalServerWatch> {
        match self {
            Self::Running(watch) => Some(watch),
            Self::Unstarted | Self::Unavailable => None,
        }
    }
}

/// Backoff after a failed top-sites query so a broken database does not stall every UI tick.
const TOP_SITES_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The sites "Jump back in" shows, and which windows showed the empty page at the last tick.
#[derive(Default)]
pub(crate) struct TopSites {
    sites: Vec<HistoryEntry>,
    showing: BTreeSet<WindowId>,
    /// The history revision the sites were read at.
    fetched: Option<u64>,
    retry_after: Option<Instant>,
}

impl Controller {
    /// Fetches the sites again when a regular window's page becomes empty, or when history
    /// changes while one is: a visit, or a deletion whose tiles must not stay clickable.
    /// The shell refetches after 60 s instead (ui/app.ts:263) because it has no such signal.
    /// Returns whether the tiles changed.
    pub(crate) fn refresh_top_sites(&mut self) -> bool {
        let showing: BTreeSet<_> = self
            .model
            .core
            .windows()
            .iter()
            .filter(|window| window.visible_tabs().is_empty())
            .map(|window| window.id.clone())
            .collect();
        let newly = showing
            .iter()
            .any(|id| !self.top_sites.showing.contains(id));
        let revision = self.history.revision();
        let stale = self.top_sites.fetched != Some(revision);
        let throttled = self
            .top_sites
            .retry_after
            .is_some_and(|until| Instant::now() < until);
        let due = newly || (!showing.is_empty() && stale && !throttled);
        self.top_sites.showing = showing;
        if !due {
            return false;
        }
        let Some(store) = self.history.store() else {
            self.top_sites.fetched = Some(revision);
            self.top_sites.retry_after = None;
            let changed = !self.top_sites.sites.is_empty();
            self.top_sites.sites.clear();
            return changed;
        };
        let Ok(sites) = store.top_sites() else {
            self.top_sites.fetched = None;
            self.top_sites.retry_after = Some(Instant::now() + TOP_SITES_RETRY_DELAY);
            let changed = !self.top_sites.sites.is_empty();
            self.top_sites.sites.clear();
            return changed;
        };
        self.top_sites.fetched = Some(revision);
        self.top_sites.retry_after = None;
        let changed = sites != self.top_sites.sites;
        self.top_sites.sites = sites;
        changed
    }

    /// Takes the dev servers the scan found since the last tick. Returns whether they changed.
    pub(crate) fn poll_local_servers(&mut self) -> bool {
        if matches!(self.local_watch, LocalWatch::Unstarted) {
            self.local_watch = match LocalServerWatch::spawn() {
                Ok(watch) => {
                    watch.exclude(self.agents.port());
                    LocalWatch::Running(watch)
                }
                Err(error) => {
                    eprintln!("local servers: could not start the scan: {error}");
                    LocalWatch::Unavailable
                }
            };
        }
        let Some(servers) = self.local_watch.get().and_then(LocalServerWatch::take) else {
            return false;
        };
        self.local_servers = servers;
        true
    }

    pub(crate) fn local_servers(&self) -> &[LocalServer] {
        &self.local_servers
    }
}

/// `siteName` (ui/app.ts:306-314): the site's own name from its title when it has one,
/// else the host.
pub(crate) fn site_name(url: &str, title: &str) -> String {
    let full = site(url);
    let site = strip_common_suffix(full);
    let last = site.rsplit('.').next().unwrap_or(site).to_lowercase();
    let named = title_parts(title).find(|part| {
        part.chars().count() <= 18
            && part
                .to_lowercase()
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
                .contains(&last)
    });
    if let Some(named) = named {
        return named.to_owned();
    }
    if site == full {
        return site.to_owned();
    }
    let mut chars = site.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `/\.(com|org|net|io|dev|app|co)(\.[a-z]{2})?$/` removed from a host.
fn strip_common_suffix(host: &str) -> &str {
    let country = host.rsplit_once('.').and_then(|(rest, code)| {
        (code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase())).then_some(rest)
    });
    for base in [country, Some(host)].into_iter().flatten() {
        if let Some((rest, tld)) = base.rsplit_once('.')
            && ["com", "org", "net", "io", "dev", "app", "co"].contains(&tld)
        {
            return rest;
        }
    }
    host
}

/// `title.split(/\s+[-|–—·:]\s+/)`, trimmed, without empty parts.
fn title_parts(title: &str) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = title.char_indices().collect();
    let mut index = 0;
    while index < chars.len() {
        let (at, ch) = chars[index];
        if ch.is_whitespace() {
            let mut next = index;
            while next < chars.len() && chars[next].1.is_whitespace() {
                next += 1;
            }
            if next < chars.len()
                && matches!(chars[next].1, '-' | '|' | '–' | '—' | '·' | ':')
                && next + 1 < chars.len()
                && chars[next + 1].1.is_whitespace()
            {
                let mut end = next + 1;
                while end < chars.len() && chars[end].1.is_whitespace() {
                    end += 1;
                }
                parts.push(&title[start..at]);
                start = chars.get(end).map_or(title.len(), |(at, _)| *at);
                index = end;
                continue;
            }
            index = next;
            continue;
        }
        index += 1;
    }
    parts.push(&title[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

/// The greeting by the hour (ui/app.ts:257).
pub(crate) fn greeting(hour: u32) -> &'static str {
    match hour {
        0..5 => "Up late?",
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    }
}

/// `.live-dot` (ui/styles.css:897-906): a green dot in a faint ring.
/// The keys of the first Space's fixed binding (`fixed_shortcuts`).
fn space_keys() -> Option<String> {
    shortcuts::fixed_shortcuts(platform())
        .into_iter()
        .find(|(_, id)| id == "selectSpace:1")
        .map(|(keys, _)| shortcuts::display_keys(Some(&keys), platform()))
}

pub(crate) fn live_dot() -> Div {
    let green = palette().green;
    div()
        .flex_none()
        .size(px(tokens::LIVE_DOT))
        .mx(px(tokens::LIVE_DOT_MARGIN))
        .rounded_full()
        .bg(green)
        .shadow(vec![gpui::BoxShadow {
            color: tokens::mix(
                green,
                tokens::LIVE_DOT_RING_SHARE / 100.,
                gpui::Rgba::default(),
            )
            .into(),
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(tokens::LIVE_DOT_RING),
            inset: false,
        }])
}

impl Shell {
    /// `#empty-state`: drawn where the page would be, as a card the same shape as a page.
    pub(crate) fn empty_page(
        &self,
        state: &Controller,
        space: &Space,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let palette = palette();
        let incognito = state.model.is_incognito(&self.id);
        let keys = |id: &str| {
            shortcuts::display_keys(
                shortcuts::keys_for(&state.model.core.settings.keybindings, id, platform()),
                platform(),
            )
        };
        let space_chip = div()
            .flex()
            .items_center()
            .gap(px(tokens::EMPTY_SPACE_GAP))
            .py(px(tokens::EMPTY_SPACE_PAD_Y))
            .px(px(tokens::EMPTY_SPACE_PAD_X))
            .rounded_full()
            .bg(palette.fill_subtle)
            .shadow(vec![hairline(palette.line)])
            .text_size(px(tokens::TEXT_SM))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.text_2)
            .when_some(
                space.emoji.clone().filter(|e| !e.is_empty()),
                |chip, emoji| {
                    chip.child(div().text_size(px(tokens::EMPTY_SPACE_EMOJI)).child(emoji))
                },
            )
            .child(space.name.clone());
        let new_tab_keys = keys("newTab");
        let search = div()
            .id("empty-new-tab")
            .flex()
            .items_center()
            .gap(px(tokens::EMPTY_SEARCH_GAP))
            .w_full()
            .h(px(tokens::EMPTY_SEARCH))
            .pl(px(tokens::EMPTY_SEARCH_PAD_LEFT))
            .pr(px(tokens::EMPTY_SEARCH_PAD_RIGHT))
            .rounded(px(tokens::EMPTY_SEARCH_RADIUS))
            .bg(palette.surface)
            .shadow(palette.shadow_pop())
            .text_color(palette.text_3)
            .text_size(px(tokens::TEXT_LG))
            .cursor_pointer()
            .aria_label("Search or enter a URL")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.open_command(Command::NewTab, window, cx);
            }))
            .child(icon("search", tokens::EMPTY_SEARCH_ICON, palette.text_3))
            .child(div().flex_1().child("Search or enter a URL"))
            .when(!new_tab_keys.is_empty(), |search| {
                search.child(kbd(new_tab_keys))
            });

        let top_sites = if incognito {
            &[][..]
        } else {
            &state.top_sites.sites[..]
        };
        let partition = state.model.window_partition(&self.id, &space.id);
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .w_full()
            .max_w(px(tokens::EMPTY_BODY))
            .pt(px(tokens::EMPTY_BODY_PAD_TOP))
            .pb(px(tokens::EMPTY_BODY_PAD_BOTTOM))
            .child(space_chip)
            .child(
                div()
                    .mt(px(tokens::EMPTY_TITLE_TOP))
                    .mb(px(tokens::EMPTY_TITLE_BOTTOM))
                    .text_size(px(tokens::EMPTY_TITLE))
                    .font_weight(FontWeight(tokens::EMPTY_TITLE_WEIGHT))
                    .child(greeting(chrono::Local::now().hour())),
            )
            .child(search);
        if !top_sites.is_empty() {
            let mut grid = div()
                .grid()
                .grid_cols(tokens::TOP_SITE_COLUMNS as u16)
                .gap(px(tokens::TOP_SITES_GAP));
            for (index, entry) in top_sites.iter().enumerate() {
                let url = entry.url.clone();
                let image = state.site_favicon(&partition, &entry.url);
                grid = grid.child(
                    div()
                        .id(("top-site", index))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(tokens::TOP_SITE_GAP))
                        .min_w_0()
                        .pt(px(tokens::TOP_SITE_PAD_TOP))
                        .px(px(tokens::TOP_SITE_PAD_X))
                        .pb(px(tokens::TOP_SITE_PAD_BOTTOM))
                        .rounded(px(tokens::RADIUS_LG))
                        .cursor_pointer()
                        .hover(|tile| tile.bg(palette.fill_hover))
                        .aria_label(SharedString::from(entry.url.clone()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, Action::Open(url.clone()), cx);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(tokens::TOP_SITE_ICON))
                                .rounded(px(tokens::TOP_SITE_ICON_RADIUS))
                                .bg(palette.surface)
                                .shadow(palette.shadow_row())
                                .child(self.site_icon(
                                    image,
                                    &entry.url,
                                    (
                                        tokens::TOP_SITE_FAVICON,
                                        tokens::TOP_SITE_FAVICON_RADIUS,
                                        tokens::TOP_SITE_LETTER,
                                    ),
                                )),
                        )
                        .child(
                            div()
                                .max_w_full()
                                .truncate()
                                .text_size(px(tokens::TEXT_SM))
                                .text_color(palette.text_2)
                                .child(site_name(&entry.url, &entry.title)),
                        ),
                );
            }
            body = body.child(empty_section("Jump back in").child(grid));
        }
        let servers = state.local_servers();
        if !servers.is_empty() {
            let chips = div()
                .flex()
                .flex_wrap()
                .gap(px(tokens::LOCAL_CHIPS_GAP))
                .children(servers.iter().map(|server| {
                    let url = server.url.clone();
                    div()
                        .id(("local-chip", usize::from(server.port)))
                        .flex()
                        .items_center()
                        .gap(px(tokens::LOCAL_CHIP_GAP))
                        .h(px(tokens::LOCAL_CHIP))
                        .px(px(tokens::LOCAL_CHIP_PAD))
                        .rounded_full()
                        .bg(palette.surface)
                        .shadow(palette.shadow_row())
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .aria_label(SharedString::from(server.url.clone()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, Action::Open(url.clone()), cx);
                        }))
                        .child(live_dot())
                        .child(server.name.clone())
                        .child(port_label(server.port))
                }));
            body = body.child(empty_section("Running locally").child(chips));
        }

        // `.empty-hints` (ui/index.html:185-191). A hint whose command has no keys hides, as
        // the shell hides it; the rewrite has no responsive view, so that hint is left out.
        let hint = |keys: String, label: &'static str| {
            (!keys.is_empty()).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::EMPTY_HINT_GAP))
                    .child(kbd(keys))
                    .child(label)
                    .into_any_element()
            })
        };
        // Space selection is a fixed binding, so like the shell's hint it always shows.
        let switch_space = space_keys().map(|space_keys| {
            div()
                .flex()
                .items_center()
                .gap(px(tokens::EMPTY_HINT_GAP))
                .child(kbd(space_keys))
                .child("–")
                .child(kbd("9"))
                .child("Switch Space")
                .into_any_element()
        });
        let hints: Vec<AnyElement> = [
            hint(keys("openLocation"), "Open location"),
            switch_space,
            hint(keys("reopenTab"), "Reopen closed tab"),
            hint(keys("shortcuts"), "All shortcuts"),
        ]
        .into_iter()
        .flatten()
        .collect();

        div()
            .id("empty-state")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .overflow_y_scroll()
            .rounded(px(tokens::PAGE_RADIUS))
            .bg(tokens::mix(
                palette.surface,
                tokens::EMPTY_SURFACE_SHARE / 100.,
                palette.tint,
            ))
            .shadow({
                let mut shadow = vec![ring(palette.line, tokens::EMPTY_HAIRLINE)];
                shadow.extend(palette.shadow_row());
                shadow
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .justify_center()
                    .px(px(tokens::EMPTY_BODY_MARGIN / 2.))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_y(px(tokens::EMPTY_HINTS_GAP_Y))
                    .gap_x(px(tokens::EMPTY_HINTS_GAP_X))
                    .pt(px(tokens::EMPTY_HINTS_PAD_TOP))
                    .px(px(tokens::EMPTY_HINTS_PAD_X))
                    .pb(px(tokens::EMPTY_HINTS_PAD_BOTTOM))
                    .text_size(px(tokens::TEXT_XS))
                    .text_color(palette.text_3)
                    .children(hints),
            )
    }
}

/// `.empty-section` and its header (ui/styles.css:1712-1728).
fn empty_section(title: &'static str) -> Div {
    div().w_full().mt(px(tokens::EMPTY_SECTION_TOP)).child(
        div()
            .mx(px(tokens::EMPTY_HEADER_PAD))
            .mb(px(tokens::EMPTY_HEADER_BOTTOM))
            .text_size(px(tokens::TEXT_XS))
            .font_weight(FontWeight(tokens::EMPTY_HEADER_WEIGHT))
            .text_color(palette().text_3)
            .child(title.to_uppercase()),
    )
}

/// `.port` (ui/styles.css:908-911, 1800-1804): `:5173` in the mono face.
pub(crate) fn port_label(port: u16) -> Div {
    div()
        .flex_none()
        .font_family(tokens::MONO)
        .text_size(px(tokens::TEXT_XS))
        .text_color(palette().text_3)
        .line_height(relative(tokens::LINE_HEIGHT))
        .child(format!(":{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_back_in_follows_history_as_it_is_visited_and_deleted() {
        use crate::history_view::ClearRange;
        use flecha_cef::Partition;

        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        assert!(
            controller
                .model
                .core
                .focused_window()
                .visible_tabs()
                .is_empty()
        );
        let shown = |controller: &Controller| -> Vec<String> {
            controller
                .top_sites
                .sites
                .iter()
                .map(|site| site.url.clone())
                .collect()
        };
        let visit = |controller: &mut Controller, url: &str| {
            controller
                .history
                .record(true, Some(&Partition::Shared), url, "Site", 1)
                .unwrap()
                .expect("the visit is saved");
        };

        controller.refresh_top_sites();
        assert!(shown(&controller).is_empty());

        // A visit while the page is empty shows at once; an unchanged history reads nothing.
        visit(&mut controller, "https://kept.test/");
        visit(&mut controller, "https://deleted.test/");
        assert!(controller.refresh_top_sites());
        assert_eq!(shown(&controller).len(), 2);
        assert!(!controller.refresh_top_sites());

        // Deleting a page or clearing history takes its tile away on the next tick.
        controller
            .act(
                &window,
                Action::DeleteHistory("https://deleted.test/".into()),
            )
            .unwrap();
        assert!(controller.refresh_top_sites());
        assert_eq!(shown(&controller), ["https://kept.test/"]);

        controller
            .act(&window, Action::ClearHistory(ClearRange::All))
            .unwrap();
        assert!(controller.refresh_top_sites());
        assert!(shown(&controller).is_empty());
    }

    #[test]
    fn the_switch_space_hint_shows_the_fixed_space_keys() {
        let keys = space_keys().expect("Space selection has a fixed binding");
        assert!(keys.ends_with('1'), "{keys}");
    }

    #[test]
    fn tiles_are_named_as_the_shell_names_them() {
        assert_eq!(site_name("https://www.figma.com/files", "Figma"), "Figma");
        assert_eq!(
            site_name("https://github.com/x", "Pull requests · GitHub"),
            "GitHub"
        );
        assert_eq!(
            site_name("https://news.ycombinator.com/", "Hacker News"),
            "News.ycombinator"
        );
        assert_eq!(site_name("https://x.com/home", "Home / X"), "Home / X");
        assert_eq!(site_name("https://bbc.co.uk/", "Home - BBC"), "BBC");
        assert_eq!(site_name("http://localhost:3000/", "Dev"), "localhost:3000");
        assert_eq!(site_name("https://example.test/", ""), "example.test");
    }

    #[test]
    fn titles_split_on_spaced_separators_only() {
        let parts: Vec<_> = title_parts("A - B | C–D :E  ·  F").collect();
        assert_eq!(parts, ["A", "B", "C–D :E", "F"]);
    }

    #[test]
    fn greetings_follow_the_shells_hours() {
        assert_eq!(greeting(0), "Up late?");
        assert_eq!(greeting(4), "Up late?");
        assert_eq!(greeting(5), "Good morning");
        assert_eq!(greeting(11), "Good morning");
        assert_eq!(greeting(12), "Good afternoon");
        assert_eq!(greeting(17), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(greeting(23), "Good evening");
    }
}
