//! The shell's colors: Dark and Light palettes, `Theme::System` resolved from the window's
//! appearance, and incognito's own violet palette whatever the theme.
//!
//! `Shell::render` picks its window's palette before it builds any element. GPUI lays out and
//! paints one window at a time on this thread, so elements built later in the same draw (list
//! rows, deferred overlays) read the same palette through [`palette`].

use std::cell::Cell;

use flecha_core::Theme;
use gpui::{Rgba, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Palette {
    /// The window behind everything.
    pub(crate) background: Rgba,
    /// Behind a page before its first frame.
    pub(crate) page: Rgba,
    pub(crate) sidebar: Rgba,
    /// Overlay panels: the command bar, Settings, History.
    pub(crate) panel: Rgba,
    /// Raised chips, notices, banners and the Peek frame.
    pub(crate) surface: Rgba,
    /// Text fields and cards set into a panel.
    pub(crate) inset: Rgba,
    pub(crate) border: Rgba,
    pub(crate) divider: Rgba,
    pub(crate) text: Rgba,
    pub(crate) text_secondary: Rgba,
    pub(crate) muted: Rgba,
    /// A menu item that does nothing here.
    pub(crate) disabled: Rgba,
    /// The selected button, tab or row.
    pub(crate) accent: Rgba,
    /// The caret and a focused field's underline.
    pub(crate) accent_strong: Rgba,
    pub(crate) selection: Rgba,
    /// A button under the pointer, and the shown favorite.
    pub(crate) hover: Rgba,
    /// A list row under the pointer or selected, and list borders.
    pub(crate) row: Rgba,
    /// Errors and destructive confirmations.
    pub(crate) destructive: Rgba,
    /// The dimmed page behind an overlay.
    pub(crate) backdrop: Rgba,
}

impl Palette {
    pub(crate) fn dark() -> Self {
        Self {
            background: rgb(0x161c20),
            page: rgb(0x171e22),
            sidebar: rgb(0x202a30),
            panel: rgb(0x1d2a32),
            surface: rgb(0x26343d),
            inset: rgb(0x243640),
            border: rgb(0x405863),
            divider: rgb(0x30404a),
            text: rgb(0xe2edf1),
            text_secondary: rgb(0xc0d1dc),
            muted: rgb(0xa4b5c3),
            disabled: rgb(0x8b9ba4),
            accent: rgb(0x27547b),
            accent_strong: rgb(0x73b4e4),
            selection: rgb(0x285783),
            hover: rgb(0x37545e),
            row: rgb(0x304855),
            destructive: rgb(0x542f31),
            backdrop: rgba(0x00000088),
        }
    }

    pub(crate) fn light() -> Self {
        Self {
            background: rgb(0xf3f5f7),
            page: rgb(0xffffff),
            sidebar: rgb(0xe4e9ed),
            panel: rgb(0xfbfcfd),
            surface: rgb(0xd9e1e7),
            inset: rgb(0xeef2f5),
            border: rgb(0xb4c2cc),
            divider: rgb(0xcfd8df),
            text: rgb(0x1a2227),
            text_secondary: rgb(0x34434d),
            muted: rgb(0x4a5862),
            disabled: rgb(0x7a8791),
            accent: rgb(0xc4daf0),
            accent_strong: rgb(0x1f6fb5),
            selection: rgb(0xb5d3f3),
            hover: rgb(0xcbd6de),
            row: rgb(0xd7e0e7),
            destructive: rgb(0xf3cfd0),
            backdrop: rgba(0x00000044),
        }
    }

    /// Dark, with the window and sidebar in the incognito Space's darkened violet.
    pub(crate) fn incognito() -> Self {
        Self {
            background: rgb(crate::incognito::BACKGROUND),
            sidebar: rgb(crate::incognito::SIDEBAR),
            ..Self::dark()
        }
    }

    /// The palette of a window with this theme and appearance. Incognito windows always look
    /// incognito.
    pub(crate) fn resolve(theme: Theme, appearance: WindowAppearance, incognito: bool) -> Self {
        if incognito {
            return Self::incognito();
        }
        let dark = match theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        if dark { Self::dark() } else { Self::light() }
    }
}

thread_local! {
    static CURRENT: Cell<Palette> = Cell::new(Palette::dark());
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

    #[test]
    fn light_muted_text_reads_on_every_light_background() {
        let light = Palette::light();
        for (name, background) in [
            ("background", light.background),
            ("page", light.page),
            ("sidebar", light.sidebar),
            ("panel", light.panel),
            ("surface", light.surface),
            ("inset", light.inset),
            ("accent", light.accent),
            ("selection", light.selection),
            ("hover", light.hover),
            ("row", light.row),
            ("destructive", light.destructive),
        ] {
            // WCAG AA for body text.
            let ratio = contrast(light.muted, background);
            assert!(ratio >= 4.5, "muted on {name} is {ratio:.2}:1");
        }
    }

    #[test]
    fn dark_and_light_ignore_the_system_appearance() {
        for appearance in APPEARANCES {
            assert_eq!(
                Palette::resolve(Theme::Dark, appearance, false),
                Palette::dark()
            );
            assert_eq!(
                Palette::resolve(Theme::Light, appearance, false),
                Palette::light()
            );
        }
    }

    #[test]
    fn system_follows_the_window_appearance() {
        for (appearance, expected) in [
            (WindowAppearance::Light, Palette::light()),
            (WindowAppearance::VibrantLight, Palette::light()),
            (WindowAppearance::Dark, Palette::dark()),
            (WindowAppearance::VibrantDark, Palette::dark()),
        ] {
            assert_eq!(
                Palette::resolve(Theme::System, appearance, false),
                expected,
                "{appearance:?}"
            );
        }
    }

    #[test]
    fn incognito_keeps_its_violet_palette_under_every_theme() {
        let incognito = Palette::incognito();
        assert_eq!(incognito.background, rgb(crate::incognito::BACKGROUND));
        assert_eq!(incognito.sidebar, rgb(crate::incognito::SIDEBAR));
        for theme in [Theme::System, Theme::Light, Theme::Dark] {
            for appearance in APPEARANCES {
                assert_eq!(Palette::resolve(theme, appearance, true), incognito);
            }
        }
    }

    #[test]
    fn the_palettes_differ_where_the_window_shows_them() {
        let (dark, light) = (Palette::dark(), Palette::light());
        for (name, dark, light) in [
            ("background", dark.background, light.background),
            ("sidebar", dark.sidebar, light.sidebar),
            ("panel", dark.panel, light.panel),
            ("text", dark.text, light.text),
            ("muted", dark.muted, light.muted),
            ("accent", dark.accent, light.accent),
        ] {
            assert_ne!(dark, light, "{name}");
        }
    }

    #[test]
    fn a_window_draws_with_the_palette_it_set() {
        draw_with(Palette::light());
        assert_eq!(palette(), Palette::light());
        draw_with(Palette::incognito());
        assert_eq!(palette(), Palette::incognito());
    }
}
