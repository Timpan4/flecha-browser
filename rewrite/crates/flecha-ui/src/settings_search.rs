//! Settings search: every pane, setting and shortcut, found by any word (ui/settings.ts:67-170).

use flecha_core::shortcuts::{self, Keybindings, SHORTCUTS};
use gpui::{
    ClickEvent, Context, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

use super::{NAV, SettingsTab, agents, nav_icon};
use crate::shortcuts::platform;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Shell, hairline};

/// The most results the shell lists (`matches.slice(0, 40)`, ui/settings.ts:126).
const MOST_RESULTS: usize = 40;

/// The settings each pane draws, under the card that holds them, so search can find a setting
/// by its name or its card's. A test checks every name is still drawn by its pane.
const ROWS: &[(SettingsTab, &str, &str)] = &[
    (SettingsTab::General, "Appearance", ""),
    (SettingsTab::General, "System", "Appearance"),
    (SettingsTab::General, "Light", "Appearance"),
    (SettingsTab::General, "Dark", "Appearance"),
    (SettingsTab::General, "Sidebar width", "Appearance"),
    (
        SettingsTab::General,
        "Sidebar in page fullscreen",
        "Appearance",
    ),
    (SettingsTab::General, "Tabs", ""),
    (
        SettingsTab::General,
        "Open links from pinned tabs in Peek",
        "Tabs",
    ),
    (
        SettingsTab::General,
        "Open external links in Flechinha",
        "Tabs",
    ),
    (SettingsTab::General, "Auto-archive after (hours)", "Tabs"),
    (SettingsTab::General, "Ad blocking", ""),
    (SettingsTab::General, "Block ads", "Ad blocking"),
    (SettingsTab::General, "Add site exception", "Ad blocking"),
    (SettingsTab::General, "Search engine", ""),
    (
        SettingsTab::General,
        "Autocomplete searches and URLs",
        "Search engine",
    ),
    (SettingsTab::General, "Downloads", ""),
    (SettingsTab::General, "Download folder", "Downloads"),
    (
        SettingsTab::General,
        "Ask where to save each download",
        "Downloads",
    ),
    (SettingsTab::General, "Developer environments", ""),
    (
        SettingsTab::General,
        "Add project",
        "Developer environments",
    ),
    (SettingsTab::General, "Site permissions", ""),
    (SettingsTab::TabsSpaces, "Search tabs and folders", ""),
    (SettingsTab::Import, "Installed browsers", ""),
    (SettingsTab::Import, "Import from file", ""),
    (SettingsTab::Import, "Pre-import backup", ""),
    (SettingsTab::Boosts, "New Boost", ""),
    (SettingsTab::Boosts, "Saved Boosts", ""),
    (
        SettingsTab::Extensions,
        "Chrome Web Store",
        "Managed installation",
    ),
    (
        SettingsTab::Extensions,
        "Developer mode",
        "Unpacked extensions",
    ),
    (SettingsTab::Agents, agents::SWITCH.0, agents::SWITCH.1),
    (SettingsTab::Agents, agents::CLAUDE_CODE, "Copy Command"),
    (
        SettingsTab::Agents,
        agents::OTHER_CLIENTS.0,
        agents::OTHER_CLIENTS.1,
    ),
    (SettingsTab::Agents, agents::TOKEN.0, agents::TOKEN.1),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hit {
    pub(super) tab: SettingsTab,
    pub(super) title: String,
    pub(super) detail: String,
    /// A setting rather than the pane itself.
    row: bool,
}

/// A pane's opening paragraph, which its own hit searches as well.
fn about(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::Agents => agents::LEAD,
        _ => "",
    }
}

fn index(bindings: &Keybindings) -> Vec<Hit> {
    let mut hits = Vec::new();
    for tab in NAV
        .iter()
        .flat_map(|(_, panes)| panes.iter().map(|pane| pane.0))
    {
        hits.push(Hit {
            tab,
            title: tab.label().into(),
            detail: about(tab).into(),
            row: false,
        });
        hits.extend(
            ROWS.iter()
                .filter(|(pane, ..)| *pane == tab)
                .map(|&(tab, title, detail)| Hit {
                    tab,
                    title: title.into(),
                    detail: detail.into(),
                    row: true,
                }),
        );
        if tab == SettingsTab::Shortcuts {
            hits.extend(SHORTCUTS.iter().map(|command| {
                Hit {
                    tab,
                    title: command.label.into(),
                    detail: shortcuts::keys_for(bindings, command.id, platform())
                        .map(|keys| shortcuts::display_keys(Some(keys), platform()))
                        .unwrap_or_default(),
                    row: true,
                }
            }));
        }
    }
    hits
}

/// The hits with every word of `query`: settings whose name matches first, then those whose
/// description does, then whole panes (ui/settings.ts:101-111).
pub(super) fn find(query: &str, bindings: &Keybindings) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let all = |text: &str| words.iter().all(|word| text.contains(word.as_str()));
    let mut matches: Vec<Hit> = index(bindings)
        .into_iter()
        .filter(|hit| {
            all(&format!("{} {} {}", hit.title, hit.detail, hit.tab.label()).to_lowercase())
        })
        .collect();
    matches.sort_by_key(|hit| {
        if all(&hit.title.to_lowercase()) {
            0
        } else if hit.row {
            1
        } else {
            2
        }
    });
    matches
}

impl Hit {
    /// Where the hit is: its pane and description (ui/settings.ts:121-123).
    fn place(&self) -> String {
        let pane = if self.row {
            self.tab.label().to_owned()
        } else {
            format!("{} pane", self.title)
        };
        if self.detail.is_empty() {
            pane
        } else {
            format!("{pane} · {}", self.detail)
        }
    }
}

impl Shell {
    /// `#search-results` (ui/settings.css:1368-1410).
    pub(super) fn search_results(&self, query: &str, matches: &[Hit], cx: &Context<Self>) -> Div {
        let palette = palette();
        if matches.is_empty() {
            return div()
                .my(px(tokens::NO_RESULTS_MARGIN))
                .flex()
                .justify_center()
                .text_color(palette.text_3)
                .child(format!("No settings match “{}”.", query.trim()));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::SEARCH_RESULTS_GAP))
            .children(
                matches
                    .iter()
                    .take(MOST_RESULTS)
                    .enumerate()
                    .map(|(i, hit)| {
                        let (glyph, tint) = NAV
                            .iter()
                            .flat_map(|(_, panes)| panes.iter())
                            .find(|pane| pane.0 == hit.tab)
                            .map_or(("settings", "gray"), |pane| (pane.1, pane.2));
                        let tab = hit.tab;
                        div()
                            .id(("settings-result", i))
                            .flex()
                            .items_center()
                            .gap(px(tokens::RESULT_GAP))
                            .py(px(tokens::RESULT_PAD_Y))
                            .px(px(tokens::RESULT_PAD_X))
                            .rounded(px(tokens::RADIUS_MD))
                            .cursor_pointer()
                            .map(|result| {
                                let found = |result: gpui::Stateful<Div>| {
                                    result.bg(palette.card).shadow(vec![gpui::BoxShadow {
                                        inset: false,
                                        ..hairline(palette.card_line)
                                    }])
                                };
                                if i == 0 {
                                    found(result)
                                } else {
                                    result.hover(|result| {
                                        result.bg(palette.card).shadow(vec![gpui::BoxShadow {
                                            inset: false,
                                            ..hairline(palette.card_line)
                                        }])
                                    })
                                }
                            })
                            .child(nav_icon(glyph, tint))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(hit.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(tokens::TEXT_SM))
                                            .text_color(palette.text_3)
                                            .child(hit.place()),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(panel) = this.settings_mut() {
                                    panel.select(tab);
                                    cx.notify();
                                }
                            }))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_indexed_setting_is_still_drawn_by_its_pane() {
        let source = |tab| match tab {
            SettingsTab::General => include_str!("settings_general.rs"),
            SettingsTab::TabsSpaces => include_str!("settings_tabs.rs"),
            SettingsTab::Import => include_str!("settings_import.rs"),
            SettingsTab::Extensions => include_str!("settings_extensions.rs"),
            SettingsTab::Boosts => include_str!("settings_boosts.rs"),
            SettingsTab::Agents | SettingsTab::Shortcuts => "",
        };
        for &(tab, title, _) in ROWS {
            if matches!(tab, SettingsTab::Agents) {
                continue;
            }
            assert!(
                source(tab).contains(&format!("\"{title}\"")),
                "{tab:?} no longer draws {title:?}"
            );
        }
    }

    #[test]
    fn names_rank_before_descriptions_and_panes() {
        let bindings = Keybindings::default();
        let found = find("token", &bindings);
        assert_eq!(found[0].title, agents::TOKEN.0);
        assert!(found.iter().all(|hit| hit.tab == SettingsTab::Agents));

        // "agents" names the pane and appears in the descriptions of its settings.
        let found = find("AGENTS", &bindings);
        let ranks: Vec<_> = found
            .iter()
            .map(|hit| (hit.title.as_str(), hit.row))
            .collect();
        assert_eq!(ranks.first(), Some(&("Coding Agents", false)));
        assert!(ranks[1..].iter().all(|(_, row)| *row));

        // Every word has to appear, in any order.
        let found = find("download ask", &bindings);
        assert_eq!(
            found
                .iter()
                .map(|hit| hit.title.as_str())
                .collect::<Vec<_>>(),
            ["Ask where to save each download"]
        );
        assert_eq!(found[0].place(), "General · Downloads");
        assert!(find("   ", &bindings).is_empty());
        assert!(find("zzzz", &bindings).is_empty());
    }

    #[test]
    fn shortcuts_are_found_by_name_with_their_keys() {
        let bindings = Keybindings::default();
        let first = SHORTCUTS[0];
        let found = find(first.label, &bindings);
        let hit = found
            .iter()
            .find(|hit| hit.tab == SettingsTab::Shortcuts && hit.title == first.label)
            .expect("the shortcut is indexed");
        assert_eq!(
            hit.detail,
            shortcuts::keys_for(&bindings, first.id, platform())
                .map(|keys| shortcuts::display_keys(Some(keys), platform()))
                .unwrap_or_default()
        );
    }
}
