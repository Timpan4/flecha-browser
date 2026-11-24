//! The shell's colours: the shipping shell's Light and Dark tokens (see [`crate::tokens`]),
//! `Theme::System` resolved from the window's appearance, and the active Space's tint behind
//! the sidebar. An incognito window is always Dark, as the shipping shell's are.
//!
//! `Shell::render` picks its window's palette before it builds any element. GPUI lays out and
//! paints one window at a time on this thread, so elements built later in the same draw (list
//! rows, deferred overlays) read the same palette through [`palette`].

use std::cell::Cell;

use flecha_core::Theme;
use gpui::{BoxShadow, Rgba, WindowAppearance};

use crate::tokens::{self, color, mix, parse_color, space_tint};

/// The shipping tokens of one theme, under their CSS names, plus the Space's tint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    /// The active Space's colour as the sidebar paints it (`--space-color`, `spaceTint`).
    pub(crate) tint: Rgba,
    pub(crate) ink: Rgba,
    pub(crate) paper: Rgba,
    pub(crate) blue: Rgba,
    pub(crate) text: Rgba,
    pub(crate) text_2: Rgba,
    pub(crate) text_3: Rgba,
    /// Fills on the Space tint: a control under the pointer, the selected row, a resting chip.
    pub(crate) fill_hover: Rgba,
    pub(crate) fill_active: Rgba,
    pub(crate) fill_subtle: Rgba,
    /// Hairlines on the Space tint.
    pub(crate) line: Rgba,
    /// Floating surfaces (the command bar, menus, popovers) and what sits on them.
    pub(crate) surface: Rgba,
    pub(crate) surface_2: Rgba,
    pub(crate) surface_line: Rgba,
    pub(crate) selection: Rgba,
    pub(crate) selection_text: Rgba,
    pub(crate) focus: Rgba,
    pub(crate) green: Rgba,
    pub(crate) amber: Rgba,
    pub(crate) red: Rgba,
    pub(crate) agent: Rgba,
    /// Settings: the content area behind its cards, and the cards.
    pub(crate) pane_bg: Rgba,
    pub(crate) card: Rgba,
    pub(crate) card_line: Rgba,
    /// Flechinha's one loud control, "Open in Flecha".
    pub(crate) cta: Rgba,
    pub(crate) cta_text: Rgba,
    pub(crate) cta_hover: Rgba,
    /// Behind a page before its first frame: the empty state's card,
    /// `color-mix(in srgb, var(--surface) 72%, var(--space-color))` (ui/styles.css:1639).
    pub(crate) page: Rgba,
    /// The dimmed page behind the command bar and panels (ui/styles.css:1219).
    pub(crate) backdrop: Rgba,
    /// The dimmed page behind Peek (ui/styles.css:1154).
    pub(crate) peek_backdrop: Rgba,
    /// The dimmed page behind a question (shell.css:150).
    pub(crate) dialog_backdrop: Rgba,
    /// A selected command bar result's icon chip and address (ui/styles.css:1346, 1364).
    pub(crate) selected_chip: Rgba,
    pub(crate) selected_sub: Rgba,
}

/// The sidebar's colour when a Space has none it can use (ui/styles.css:35).
pub(crate) const DEFAULT_TINT: &str = "#cdd9f7";

impl Palette {
    pub(crate) fn new(dark: bool, space_color: &str) -> Self {
        let token = |name| color(name, dark);
        let tint = space_tint(
            parse_color(space_color)
                .filter(|_| space_color.len() == 7)
                .unwrap_or_else(|| color("#sidebar (no Space colour)", dark)),
            dark,
        );
        let surface = token("--surface");
        Self {
            dark,
            tint,
            ink: token("--ink"),
            paper: token("--paper"),
            blue: token("--blue"),
            text: token("--text"),
            text_2: token("--text-2"),
            text_3: token("--text-3"),
            fill_hover: token("--fill-hover"),
            fill_active: token("--fill-active"),
            fill_subtle: token("--fill-subtle"),
            line: token("--line"),
            surface,
            surface_2: token("--surface-2"),
            surface_line: token("--surface-line"),
            selection: token("--selection"),
            selection_text: token("--selection-text"),
            focus: token("--focus"),
            green: token("--green"),
            amber: token("--amber"),
            red: token("--red"),
            agent: token("--agent"),
            pane_bg: token("--pane-bg"),
            card: token("--card"),
            card_line: token("--card-line"),
            cta: token("--cta"),
            cta_text: token("--cta-text"),
            cta_hover: token("--cta-hover"),
            page: mix(surface, 0.72, tint),
            backdrop: token("#palette backdrop"),
            peek_backdrop: token("#peek backdrop"),
            dialog_backdrop: token(".shell-dialog::backdrop"),
            selected_chip: token("#palette-results .item.selected .item-icon"),
            selected_sub: token("#palette-results .item.selected .sub"),
        }
    }

    /// The palette of a window with this theme and appearance, whose active Space has
    /// `space_color`. Incognito windows are always Dark (chromium/shell/flechaApi.ts:23).
    pub(crate) fn resolve(
        theme: Theme,
        appearance: WindowAppearance,
        incognito: bool,
        space_color: &str,
    ) -> Self {
        let dark = incognito
            || match theme {
                Theme::Dark => true,
                Theme::Light => false,
                Theme::System => matches!(
                    appearance,
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                ),
            };
        Self::new(dark, space_color)
    }

    pub(crate) fn shadow_row(&self) -> Vec<BoxShadow> {
        tokens::shadow_row(self.dark)
    }

    pub(crate) fn shadow_pop(&self) -> Vec<BoxShadow> {
        tokens::shadow_pop(self.dark)
    }

    /// The letter placeholder of a site without an icon:
    /// `hsl(var(--hue, 210) 42% 48%)` (ui/styles.css:755).
    pub(crate) fn letter(hue: u32) -> Rgba {
        gpui::hsla(hue as f32 / 360., 0.42, 0.48, 1.).into()
    }
}

thread_local! {
    static CURRENT: Cell<Palette> = Cell::new(Palette::new(false, DEFAULT_TINT));
}

/// The palette of the window being drawn.
pub(crate) fn palette() -> Palette {
    CURRENT.with(Cell::get)
}

/// Makes `palette` the one the window about to be drawn uses.
pub(crate) fn draw_with(palette: Palette) {
    CURRENT.with(|current| current.set(palette));
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPEARANCES: [WindowAppearance; 4] = [
        WindowAppearance::Light,
        WindowAppearance::VibrantLight,
        WindowAppearance::Dark,
        WindowAppearance::VibrantDark,
    ];

    /// The Space colours the shipping shell offers (ui/spaceChoices.ts:3).
    fn space_colors() -> Vec<&'static str> {
        let source = crate::tokens::tests::source("ui/spaceChoices.ts");
        let line = source
            .lines()
            .find(|line| line.starts_with("export const SPACE_COLORS = ["))
            .unwrap();
        line.split('\'').skip(1).step_by(2).collect()
    }

    /// `foreground` painted over an opaque `background`.
    fn over(foreground: Rgba, background: Rgba) -> Rgba {
        let a = foreground.a;
        Rgba {
            r: foreground.r * a + background.r * (1. - a),
            g: foreground.g * a + background.g * (1. - a),
            b: foreground.b * a + background.b * (1. - a),
            a: 1.,
        }
    }

    /// WCAG 2.x contrast ratio.
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let luminance = |color: Rgba| {
            let linear = |c: f32| {
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Every opaque ground text sits on, in either theme and for every offered Space colour.
    fn grounds(dark: bool) -> Vec<(String, Rgba)> {
        let mut grounds = Vec::new();
        for space in space_colors() {
            let palette = Palette::new(dark, space);
            grounds.push((format!("tint {space}"), palette.tint));
            grounds.push((
                format!("selected row on {space}"),
                over(palette.fill_active, palette.tint),
            ));
            grounds.push((format!("page {space}"), palette.page));
        }
        let palette = Palette::new(dark, DEFAULT_TINT);
        for (name, ground) in [
            ("surface", palette.surface),
            ("surface-2", palette.surface_2),
            ("pane-bg", palette.pane_bg),
            ("card", palette.card),
        ] {
            grounds.push((name.to_owned(), ground));
        }
        grounds
    }

    #[test]
    fn body_text_reads_on_every_ground() {
        for dark in [false, true] {
            let palette = Palette::new(dark, DEFAULT_TINT);
            for (name, ground) in grounds(dark) {
                // WCAG AA for body text.
                let ratio = contrast(over(palette.text, ground), ground);
                assert!(ratio >= 4.5, "dark {dark}: text on {name} is {ratio:.2}:1");
            }
        }
    }

    /// The shipping `--text-2` is lighter than AA body text on the pastel tints (3.9:1 on the
    /// default one), so it gets WCAG AA's large-text and non-text minimum here.
    #[test]
    fn secondary_text_meets_the_large_text_minimum() {
        for dark in [false, true] {
            let palette = Palette::new(dark, DEFAULT_TINT);
            for (name, ground) in grounds(dark) {
                let ratio = contrast(over(palette.text_2, ground), ground);
                assert!(ratio >= 3., "dark {dark}: text-2 on {name} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn selected_text_reads_on_the_selection() {
        // WCAG 2.2 1.4.3 asks 4.5:1 of body text. The shipping Dark selection, `#fff` on
        // `#2b7fc4` (ui/tokens.css:55, 110), reaches 4.25:1: the rewrite keeps the shipping
        // value, and this test holds Light to 4.5:1 and Dark to the large-text 3:1.
        for (dark, minimum) in [(false, 4.5), (true, 3.)] {
            let palette = Palette::new(dark, DEFAULT_TINT);
            let ratio = contrast(palette.selection_text, palette.selection);
            assert!(ratio >= minimum, "dark {dark}: {ratio:.2}:1");
        }
    }

    #[test]
    fn dark_and_light_ignore_the_system_appearance() {
        for appearance in APPEARANCES {
            assert_eq!(
                Palette::resolve(Theme::Dark, appearance, false, DEFAULT_TINT),
                Palette::new(true, DEFAULT_TINT)
            );
            assert_eq!(
                Palette::resolve(Theme::Light, appearance, false, DEFAULT_TINT),
                Palette::new(false, DEFAULT_TINT)
            );
        }
    }

    #[test]
    fn system_follows_the_window_appearance() {
        for (appearance, dark) in [
            (WindowAppearance::Light, false),
            (WindowAppearance::VibrantLight, false),
            (WindowAppearance::Dark, true),
            (WindowAppearance::VibrantDark, true),
        ] {
            assert_eq!(
                Palette::resolve(Theme::System, appearance, false, DEFAULT_TINT).dark,
                dark,
                "{appearance:?}"
            );
        }
    }

    #[test]
    fn incognito_is_dark_in_its_space_tint_under_every_theme() {
        let incognito = Palette::new(true, crate::incognito::SPACE_COLOR);
        // spaceTint('#ddd5f3', true) is #323645.
        assert_eq!(incognito.tint, parse_color("#323645").unwrap());
        for theme in [Theme::System, Theme::Light, Theme::Dark] {
            for appearance in APPEARANCES {
                assert_eq!(
                    Palette::resolve(theme, appearance, true, crate::incognito::SPACE_COLOR),
                    incognito
                );
            }
        }
    }

    #[test]
    fn the_sidebar_wears_the_space_tint() {
        assert_eq!(
            Palette::new(false, "#f5e7c4").tint,
            parse_color("#f5e7c4").unwrap()
        );
        assert_ne!(
            Palette::new(false, "#f5e7c4").tint,
            Palette::new(true, "#f5e7c4").tint
        );
        assert_eq!(
            tokens::css(tokens::COLORS, "#sidebar (no Space colour)", false),
            DEFAULT_TINT
        );
        // A colour the shell cannot use falls back to the shipping default.
        assert_eq!(
            Palette::new(false, "teal").tint,
            parse_color(DEFAULT_TINT).unwrap()
        );
    }

    #[test]
    fn a_window_draws_with_the_palette_it_set() {
        draw_with(Palette::new(false, "#f5e7c4"));
        assert_eq!(palette(), Palette::new(false, "#f5e7c4"));
        draw_with(Palette::new(true, DEFAULT_TINT));
        assert_eq!(palette(), Palette::new(true, DEFAULT_TINT));
    }
}
