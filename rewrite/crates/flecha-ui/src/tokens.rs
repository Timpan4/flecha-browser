//! The shipping shell's design tokens (`chromium/shell`), with the line each value comes from.
//!
//! [`COLORS`] and [`SHADOWS`] keep each CSS value as text, so one table gives the palette, the
//! test that compares it with the shipping CSS, and the citations. The `cited!` constants are
//! the sizes and timings of the shipping rules. Paths are relative to `chromium/shell`.

use std::sync::LazyLock;
use std::time::Duration;

use gpui::{BoxShadow, Rgba, point, px};

/// A value the shipping CSS declares, on the line given, in Light and in Dark.
pub(crate) struct Css {
    /// The custom property, or the rule's selector for a literal value.
    pub(crate) name: &'static str,
    /// Where the token test finds the cited line.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only the token test reads the source")
    )]
    pub(crate) file: &'static str,
    pub(crate) light: (u32, &'static str),
    /// The value under `prefers-color-scheme: dark`, where it differs.
    pub(crate) dark: Option<(u32, &'static str)>,
}

const fn both(name: &'static str, file: &'static str, line: u32, value: &'static str) -> Css {
    Css {
        name,
        file,
        light: (line, value),
        dark: None,
    }
}

const fn themed(
    name: &'static str,
    file: &'static str,
    light: (u32, &'static str),
    dark: (u32, &'static str),
) -> Css {
    Css {
        name,
        file,
        light,
        dark: Some(dark),
    }
}

const TOKENS: &str = "ui/tokens.css";
const STYLES: &str = "ui/styles.css";
const SETTINGS: &str = "ui/settings.css";
const FLECHINHA: &str = "ui/flechinha.css";
const SHELL: &str = "shell.css";
const WINDOW_CONTROLS: &str = "ui/windowControls.css";
#[cfg(test)]
const SETTINGS_UI: &str = "settingsUi.ts";
#[cfg(test)]
const APP: &str = "ui/app.ts";
#[cfg(test)]
const FRAME_API: &str = "../patches/0010-flecha-private-extension-api.patch";
#[cfg(test)]
const SPLIT: &str = "../patches/0019-flat-split-view-and-split-ratio.patch";

pub(crate) const COLORS: &[Css] = &[
    both("--ink", TOKENS, 34, "#0b2545"),
    both("--paper", TOKENS, 35, "#eef2f6"),
    both("--blue", TOKENS, 36, "#3aa6d9"),
    themed("--text", TOKENS, (40, "#16243a"), (98, "#e6ecf3")),
    themed(
        "--text-2",
        TOKENS,
        (41, "rgba(22, 36, 58, 0.62)"),
        (99, "rgba(230, 236, 243, 0.62)"),
    ),
    themed(
        "--text-3",
        TOKENS,
        (42, "rgba(22, 36, 58, 0.42)"),
        (100, "rgba(230, 236, 243, 0.4)"),
    ),
    themed(
        "--fill-hover",
        TOKENS,
        (45, "rgba(255, 255, 255, 0.42)"),
        (102, "rgba(255, 255, 255, 0.06)"),
    ),
    themed(
        "--fill-active",
        TOKENS,
        (46, "rgba(255, 255, 255, 0.92)"),
        (103, "rgba(255, 255, 255, 0.13)"),
    ),
    themed(
        "--fill-subtle",
        TOKENS,
        (47, "rgba(255, 255, 255, 0.3)"),
        (104, "rgba(255, 255, 255, 0.05)"),
    ),
    themed(
        "--line",
        TOKENS,
        (48, "rgba(11, 37, 69, 0.1)"),
        (105, "rgba(230, 236, 243, 0.1)"),
    ),
    themed("--surface", TOKENS, (51, "#ffffff"), (107, "#161e2b")),
    themed("--surface-2", TOKENS, (52, "#f4f6f9"), (108, "#1b2433")),
    themed(
        "--surface-line",
        TOKENS,
        (53, "rgba(11, 37, 69, 0.1)"),
        (109, "rgba(230, 236, 243, 0.1)"),
    ),
    themed("--selection", TOKENS, (54, "#1f6fb5"), (110, "#2b7fc4")),
    both("--selection-text", TOKENS, 55, "#fff"),
    both("--focus", TOKENS, 56, "rgba(58, 166, 217, 0.55)"),
    both("--green", TOKENS, 59, "#1f9d5b"),
    both("--amber", TOKENS, 60, "#d98a12"),
    both("--red", TOKENS, 61, "#d93f3f"),
    both("--agent", TOKENS, 62, "#6d5bd0"),
    themed("--pane-bg", SETTINGS, (6, "#f5f6f8"), (13, "#1c2230")),
    themed("--card", SETTINGS, (7, "#ffffff"), (14, "#252c3b")),
    themed(
        "--card-line",
        SETTINGS,
        (8, "rgba(11, 37, 69, 0.08)"),
        (15, "rgba(230, 236, 243, 0.08)"),
    ),
    themed("--cta", FLECHINHA, (6, "var(--ink)"), (13, "#e6ecf3")),
    themed("--cta-text", FLECHINHA, (7, "#fff"), (14, "#0b2545")),
    themed("--cta-hover", FLECHINHA, (8, "#12345c"), (15, "#ffffff")),
    // Literal colours of single rules.
    both("#sidebar (no Space colour)", STYLES, 35, "#cdd9f7"),
    both("#palette backdrop", STYLES, 1219, "rgba(7, 18, 33, 0.22)"),
    both("#peek backdrop", STYLES, 1154, "rgba(7, 18, 33, 0.32)"),
    both(
        "#palette-results .item.selected .item-icon",
        STYLES,
        1346,
        "rgba(255, 255, 255, 0.18)",
    ),
    both(
        "#palette-results .item.selected .sub",
        STYLES,
        1364,
        "rgba(255, 255, 255, 0.75)",
    ),
    both(
        ".shell-dialog::backdrop",
        SHELL,
        150,
        "rgba(11, 37, 69, 0.18)",
    ),
    both(".window-control.close", WINDOW_CONTROLS, 26, "#ff5f57"),
    both(".window-control.minimize", WINDOW_CONTROLS, 30, "#febc2e"),
    both(".window-control.maximize", WINDOW_CONTROLS, 34, "#28c840"),
    // An unfocused window's circles, until the pointer comes.
    both(
        ".window-control idle",
        WINDOW_CONTROLS,
        39,
        "rgba(11, 37, 69, 0.18)",
    ),
    // Settings' pane icons: each a top-to-bottom gradient between two colours.
    both(".nav-icon.gray top", SETTINGS, 111, "#8e98a6"),
    both(".nav-icon.gray bottom", SETTINGS, 111, "#6b7686"),
    both(".nav-icon.orange top", SETTINGS, 127, "#f07a52"),
    both(".nav-icon.orange bottom", SETTINGS, 127, "#e4572e"),
    both(".nav-icon.teal top", SETTINGS, 438, "#3fc1c9"),
    both(".nav-icon.teal bottom", SETTINGS, 438, "#1c97a8"),
    both(".nav-icon.indigo top", SETTINGS, 640, "#6f8cf0"),
    both(".nav-icon.indigo bottom", SETTINGS, 640, "#4a64d0"),
    both(".nav-icon.amber top", SETTINGS, 123, "#f0b24a"),
    both(".nav-icon.amber bottom", SETTINGS, 123, "#d98a12"),
    both(".nav-icon.purple top", SETTINGS, 131, "#8f7cf0"),
    both(".nav-icon.purple bottom", SETTINGS, 131, "#6d5bd0"),
    both(".env-badge color", STYLES, 307, "#fff"),
    both(".nav-icon color", SETTINGS, 100, "#fff"),
    both(".switch::after background", SETTINGS, 345, "#fff"),
    both(
        ".window-control svg stroke",
        WINDOW_CONTROLS,
        47,
        "rgba(0, 0, 0, 0.62)",
    ),
];

pub(crate) const SHADOWS: &[Css] = &[
    themed(
        "--shadow-row",
        TOKENS,
        (
            88,
            "0 1px 2px rgba(11, 37, 69, 0.08), 0 0 0 0.5px rgba(11, 37, 69, 0.06)",
        ),
        (112, "0 0 0 0.5px rgba(255, 255, 255, 0.08)"),
    ),
    themed(
        "--shadow-pop",
        TOKENS,
        (
            89,
            "0 18px 50px rgba(11, 37, 69, 0.22), 0 0 0 0.5px rgba(11, 37, 69, 0.14)",
        ),
        (
            113,
            "0 18px 50px rgba(0, 0, 0, 0.5), 0 0 0 0.5px rgba(255, 255, 255, 0.12)",
        ),
    ),
    both(
        ".switch::after",
        STYLES,
        1574,
        "0 1px 2px rgba(0, 0, 0, 0.3)",
    ),
    both(
        ".window-control::before",
        WINDOW_CONTROLS,
        21,
        "inset 0 0 0 0.5px rgba(0, 0, 0, 0.18)",
    ),
    both(
        ".card",
        SETTINGS,
        224,
        "0 0 0 0.5px var(--card-line), 0 1px 2px rgba(11, 37, 69, 0.04)",
    ),
    both(
        ".button",
        SETTINGS,
        288,
        "0 0 0 0.5px var(--surface-line), 0 1px 1.5px rgba(11, 37, 69, 0.08)",
    ),
    both(
        ".nav-icon",
        SETTINGS,
        101,
        "inset 0 0 0 0.5px rgba(0, 0, 0, 0.12)",
    ),
    both("main", SETTINGS, 141, "inset 0.5px 0 0 var(--card-line)"),
    both(
        "#address.editing",
        FLECHINHA,
        138,
        "inset 0 0 0 1px var(--focus), 0 1px 2px rgba(11, 37, 69, 0.08)",
    ),
];

/// One size or timing of a shipping rule: its value, and the declaration it comes from.
#[cfg(test)]
pub(crate) struct Metric {
    pub(crate) name: &'static str,
    pub(crate) value: f32,
    pub(crate) file: &'static str,
    pub(crate) line: u32,
    pub(crate) declaration: &'static str,
}

macro_rules! cited {
    ($($(#[$doc:meta])* $name:ident = $value:literal, $file:ident : $line:literal $declaration:literal;)*) => {
        $($(#[$doc])* pub(crate) const $name: f32 = $value;)*

        #[cfg(test)]
        pub(crate) const METRICS: &[Metric] = &[$(Metric {
            name: stringify!($name),
            value: $value,
            file: $file,
            line: $line,
            declaration: $declaration,
        },)*];
    };
}

cited! {
    TEXT_XS = 11., TOKENS: 67 "--text-xs: 11px";
    TEXT_SM = 12., TOKENS: 68 "--text-sm: 12px";
    TEXT_MD = 13., TOKENS: 69 "--text-md: 13px";
    TEXT_LG = 15., TOKENS: 70 "--text-lg: 15px";
    TEXT_XL = 17., TOKENS: 71 "--text-xl: 17px";
    SPACE_1 = 4., TOKENS: 74 "--space-1: 4px";
    SPACE_2 = 8., TOKENS: 75 "--space-2: 8px";
    SPACE_3 = 12., TOKENS: 76 "--space-3: 12px";
    SPACE_4 = 16., TOKENS: 77 "--space-4: 16px";
    RADIUS_SM = 6., TOKENS: 79 "--radius-sm: 6px";
    RADIUS_MD = 8., TOKENS: 80 "--radius-md: 8px";
    RADIUS_LG = 12., TOKENS: 81 "--radius-lg: 12px";
    RADIUS_XL = 16., TOKENS: 82 "--radius-xl: 16px";
    MEDIUM_MS = 200., TOKENS: 92 "--medium: 200ms";
    /// `html, body` line height, a multiple of the font size.
    LINE_HEIGHT = 1.35, STYLES: 17 "font: 400 var(--text-md) / 1.35 var(--font)";
    /// The `svg` rule: every icon is drawn on a 16px grid.
    ICON = 16., TOKENS: 148 "width: 16px";
    ICON_STROKE = 1.5, TOKENS: 153 "stroke-width: 1.5";

    // The sidebar.
    SIDEBAR_PAD_TOP = 10., STYLES: 32 "padding: 10px 6px 8px 10px";
    SIDEBAR_PAD_RIGHT = 6., STYLES: 32 "padding: 10px 6px 8px 10px";
    SIDEBAR_PAD_BOTTOM = 8., STYLES: 32 "padding: 10px 6px 8px 10px";
    SIDEBAR_PAD_LEFT = 10., STYLES: 32 "padding: 10px 6px 8px 10px";
    TOOLBAR_GAP = 2., STYLES: 55 "gap: 2px";
    TOOLBAR_HEIGHT = 28., STYLES: 56 "height: 28px";
    TOOLBAR_BUTTON = 30., STYLES: 66 "width: 30px";
    TOOLBAR_ICON = 18., STYLES: 71 "width: 18px";
    ICON_BUTTON = 28., STYLES: 136 "width: 28px";
    URLBAR_HEIGHT = 36., STYLES: 177 "height: 36px";
    URLBAR_PAD_LEFT = 10., STYLES: 196 "padding: 0 4px 0 10px";
    URLBAR_PAD_RIGHT = 4., STYLES: 178 "padding-right: 4px";
    URL_ACTION = 26., STYLES: 203 "width: 26px";
    URL_ACTION_ICON = 15., STYLES: 222 "width: 15px";
    URLBAR_ICON = 14., STYLES: 274 "width: 14px";
    FAVORITES_GAP = 6., STYLES: 416 "gap: 6px";
    FAVORITE_HEIGHT = 44., STYLES: 433 "height: 44px";
    FAVORITE_ICON = 20., STYLES: 453 "width: 20px";
    FAVORITE_ICON_RADIUS = 5., STYLES: 455 "border-radius: 5px";
    GROUP_HEADER = 22., STYLES: 469 "height: 22px";
    GROUP_HEADER_PAD_RIGHT = 4., STYLES: 470 "padding: 0 4px 0 8px";
    GROUP_HEADER_PAD_LEFT = 8., STYLES: 470 "padding: 0 4px 0 8px";
    TODAY_HEADER = 30., STYLES: 480 "height: 30px";
    TODAY_RULE = 0.5, STYLES: 478 "border-top: 0.5px solid var(--line)";
    SPACE_HEADER = 26., STYLES: 497 "height: 26px";
    SPACE_HEADER_PAD = 8., STYLES: 498 "padding: 0 8px";
    ROW = 32., STYLES: 6 "--row: 32px";
    ROW_GAP = 10., STYLES: 582 "gap: 10px";
    ROW_MARGIN = 1., STYLES: 585 "margin: 1px 0";
    ROW_PAD_RIGHT = 6., STYLES: 586 "padding: 0 6px 0 9px";
    ROW_PAD_LEFT = 9., STYLES: 586 "padding: 0 6px 0 9px";
    ROW_CLOSE = 22., STYLES: 631 "width: 22px";
    ROW_CLOSE_ICON = 13., STYLES: 648 "width: 13px";
    FAVICON = 16., STYLES: 743 "height: 16px";
    FAVICON_RADIUS = 4., STYLES: 745 "border-radius: 4px";
    FAVICON_LETTER = 9., STYLES: 753 "font: 700 9px / 1 var(--font)";
    /// One turn of `.favicon.spinner`.
    SPINNER_MS = 800., APP: 381 "duration: 800, iterations: Infinity";
    AUDIO_MARK = 20., STYLES: 2174 "width: 20px";
    AUDIO_MARK_ICON = 14., STYLES: 2190 "width: 14px";
    /// `.split-badge svg`, `.boost-mark svg`.
    ROW_BADGE_ICON = 13., STYLES: 671 "width: 13px";
    KBD_MIN_WIDTH = 18., TOKENS: 160 "min-width: 18px";
    KBD_PAD_Y = 1., TOKENS: 161 "padding: 1px 5px";
    KBD_PAD_X = 5., TOKENS: 161 "padding: 1px 5px";
    KBD_RADIUS = 4., TOKENS: 162 "border-radius: 4px";
    KBD_LINE = 16., TOKENS: 165 "font: 500 var(--text-xs) / 16px var(--mono)";
    FOLDER_INDENT = 16., STYLES: 778 "padding-left: 16px";
    SPACE_DOT = 28., STYLES: 937 "width: 28px";
    SPACE_DOT_IDLE_OPACITY = 0.6, STYLES: 940 "opacity: 0.6";
    SPACE_INITIAL = 18., STYLES: 982 "width: 18px";
    SPACE_INITIAL_RADIUS = 5., STYLES: 984 "border-radius: 5px";
    SPACE_INITIAL_TEXT = 10., STYLES: 987 "font-size: 10px";
    ICON_BUTTON_DISABLED_OPACITY = 0.6, STYLES: 154 "opacity: 0.6";
    TEXT_BUTTON_PAD_Y = 2., STYLES: 158 "padding: 2px 6px";
    TEXT_BUTTON_PAD_X = 6., STYLES: 158 "padding: 2px 6px";
    URLBAR_PROGRESS = 2., STYLES: 264 "left bottom / 0% 2px no-repeat";
    /// How long the loading line takes to run (ui/app.ts:338).
    PROGRESS_MS = 10000., APP: 338 "const PROGRESS_MS = 10_000;";
    /// The percentage of the address the line reaches quickly, at this share of its run.
    PROGRESS_FIRST = 30., APP: 335 "{ backgroundSize: '30% 2px', offset: 0.03";
    PROGRESS_FIRST_AT = 0.03, APP: 335 "offset: 0.03";
    PROGRESS_LAST = 90., APP: 336 "{ backgroundSize: '90% 2px' }";
    FAVORITE_LETTER = 11., STYLES: 456 "font-size: 11px";
    SPACE_HEADER_GAP = 6., STYLES: 496 "gap: 6px";
    SPACE_EMOJI = 14., STYLES: 504 "font-size: 14px";
    SPACE_ISOLATED_ICON = 13., STYLES: 540 "width: 13px";
    /// `.row.in-split:not(.active)`: the active fill at this percentage, over the sidebar.
    IN_SPLIT_SHARE = 60., STYLES: 602 "color-mix(in srgb, var(--fill-active) 60%, transparent)";
    FOLDER_GUIDE = 1., STYLES: 788 "width: 1px";
    FOLDER_GUIDE_INSET = 2., STYLES: 786 "top: 2px";
    DOWNLOADS_BADGE = 15., STYLES: 996 "height: 15px";
    DOWNLOADS_BADGE_PAD = 4., STYLES: 997 "padding: 0 4px";
    DOWNLOADS_BADGE_RADIUS = 8., STYLES: 998 "border-radius: 8px";
    DOWNLOADS_BADGE_TEXT = 9., STYLES: 1001 "font: 700 9px / 15px var(--mono)";
    PRIMARY_BUTTON = 28., STYLES: 1604 "height: 28px";
    PRIMARY_BUTTON_PAD = 14., STYLES: 1605 "padding: 0 14px";

    // Popovers: downloads, site controls, the Space editor.
    POPOVER_PAD_TOP = 12., STYLES: 1039 "padding: 12px 10px 6px 14px";
    POPOVER_PAD_RIGHT = 10., STYLES: 1039 "padding: 12px 10px 6px 14px";
    POPOVER_PAD_BOTTOM = 6., STYLES: 1039 "padding: 12px 10px 6px 14px";
    POPOVER_PAD_LEFT = 14., STYLES: 1039 "padding: 12px 10px 6px 14px";
    POPOVER_LEFT = 8., STYLES: 1012 "left: 8px";
    POPOVER_RIGHT = 4., STYLES: 1013 "right: 4px";
    POPOVER_BOTTOM = 46., STYLES: 1014 "bottom: 46px";
    /// `max-height: 50vh`, a share of the window's height.
    POPOVER_MAX_HEIGHT = 50., STYLES: 1015 "max-height: 50vh";
    SITE_PANEL_TOP = 92., STYLES: 1966 "top: 92px";
    SPACE_EDITOR_TOP = 128., STYLES: 1421 "top: 128px";
    DOWNLOAD_PAD_Y = 7., STYLES: 1068 "padding: 7px 4px 7px 8px";
    DOWNLOAD_PAD_RIGHT = 4., STYLES: 1068 "padding: 7px 4px 7px 8px";
    DOWNLOAD_PAD_LEFT = 8., STYLES: 1068 "padding: 7px 4px 7px 8px";
    DOWNLOADS_LIST_PAD = 6., STYLES: 1058 "padding: 0 6px 6px";
    DOWNLOAD_ACTIONS_GAP = 2., STYLES: 1106 "gap: 2px";
    DOWNLOAD_BAR = 3., STYLES: 1130 "height: 3px";
    DOWNLOAD_BAR_GAP = 6., STYLES: 1131 "margin-top: 6px";
    DOWNLOAD_BAR_RADIUS = 2., STYLES: 1132 "border-radius: 2px";
    DOWNLOADS_EMPTY_PAD_TOP = 10., STYLES: 1145 "padding: 10px 8px 12px";
    DOWNLOADS_EMPTY_PAD_X = 8., STYLES: 1145 "padding: 10px 8px 12px";
    DOWNLOADS_EMPTY_PAD_BOTTOM = 12., STYLES: 1145 "padding: 10px 8px 12px";
    TEXT_FIELD = 32., STYLES: 1435 "height: 32px";
    TEXT_FIELD_PAD = 10., STYLES: 1436 "padding: 0 10px";
    /// The focused text field's `--focus` ring.
    FOCUS_RING = 3., STYLES: 1447 "0 0 0 3px var(--focus)";
    MULTILINE_PAD_Y = 8., SETTINGS: 409 "padding: 8px 10px";
    MULTILINE_PAD_X = 10., SETTINGS: 409 "padding: 8px 10px";
    MULTILINE_TEXT = 12., SETTINGS: 414 "font: 12px / 1.6 var(--mono)";
    MULTILINE_LINE_HEIGHT = 1.6, SETTINGS: 414 "font: 12px / 1.6 var(--mono)";
    SWITCH_WIDTH = 32., STYLES: 1556 "width: 32px";
    SWITCH_HEIGHT = 19., STYLES: 1557 "height: 19px";
    SWITCH_KNOB = 15., STYLES: 1570 "width: 15px";
    SITE_INSET = 12., STYLES: 1964 "--inset: 12px";
    SITE_FAVICON = 24., STYLES: 1980 "width: 24px";
    SWITCH_KNOB_INSET = 2., STYLES: 1568 "top: 2px";
    SWITCH_TRAVEL = 13., STYLES: 1583 "transform: translateX(13px)";
    POPOVER_FOOTER_PAD_TOP = 10., STYLES: 1590 "padding: 10px 14px 12px";
    POPOVER_FOOTER_PAD_X = 14., STYLES: 1590 "padding: 10px 14px 12px";
    POPOVER_FOOTER_PAD_BOTTOM = 12., STYLES: 1590 "padding: 10px 14px 12px";
    POPOVER_RULE = 0.5, STYLES: 1591 "border-top: 0.5px solid var(--surface-line)";
    SITE_HEADER_GAP = 10., STYLES: 1975 "gap: 10px";
    SITE_HEADER_PAD_BOTTOM = 10., STYLES: 1976 "padding: var(--inset) var(--inset) 10px";
    SITE_FAVICON_RADIUS = 6., STYLES: 1982 "border-radius: 6px";
    SITE_SHIELD_GAP = 10., STYLES: 2054 "gap: 10px";
    SITE_SHIELD_PAD_Y = 10., STYLES: 2055 "padding: 10px var(--inset)";
    SITE_SHIELD_ICON = 18., STYLES: 2061 "width: 18px";
    SITE_LABEL_PAD_TOP = 10., STYLES: 2096 "padding: 10px var(--inset) 2px";
    SITE_LABEL_PAD_BOTTOM = 2., STYLES: 2096 "padding: 10px var(--inset) 2px";
    SITE_ROW_GAP = 8., STYLES: 2103 "gap: 8px";
    SITE_ROW = 32., STYLES: 2104 "height: 32px";
    MINI_SELECT = 24., STYLES: 2114 "height: 24px";
    MINI_SELECT_PAD = 6., STYLES: 2115 "padding: 0 6px";
    ZOOM_VALUE = 24., STYLES: 2143 "height: 24px";
    /// The column the site panel's controls share on the right (`--control`).
    SITE_CONTROL = 76., STYLES: 1965 "--control: 76px";
    SITE_ACTIONS_GAP = 6., STYLES: 2010 "gap: 6px";
    SITE_ACTION_GAP = 8., STYLES: 2018 "gap: 8px";
    SITE_ACTION = 32., STYLES: 2019 "height: 32px";
    SITE_ACTION_PAD = 10., STYLES: 2021 "padding: 0 10px";
    SITE_ACTION_ICON = 14., STYLES: 2037 "width: 14px";
    ZOOM_STEP = 22., STYLES: 2125 "grid-template-columns: 22px 1fr 22px";
    ZOOM_STEP_ICON = 12., STYLES: 2138 "width: 12px";
    SITE_FOOTER_MARGIN = 6., STYLES: 2153 "margin-top: 6px";
    SITE_FOOTER_PAD_TOP = 8., STYLES: 2154 "padding: 8px var(--inset) 10px";
    SITE_FOOTER_PAD_BOTTOM = 10., STYLES: 2154 "padding: 8px var(--inset) 10px";
    SITE_FOOTER_BUTTON_GAP = 6., STYLES: 2160 "gap: 6px";
    SITE_FOOTER_ICON = 13., STYLES: 2165 "width: 13px";

    // Peek.
    PILL_GAP = 6., STYLES: 1193 "gap: 6px";
    PILL_HEIGHT = 28., STYLES: 1194 "height: 28px";
    PILL_PAD = 10., STYLES: 1195 "padding: 0 10px";
    PILL_ICON = 14., STYLES: 1206 "width: 14px";
    PEEK_BAR = 36., STYLES: 1173 "height: 36px";
    PEEK_BAR_PAD_LEFT = 14., STYLES: 1174 "padding: 0 4px 0 14px";
    PEEK_BAR_PAD_RIGHT = 4., STYLES: 1174 "padding: 0 4px 0 14px";

    // The command bar.
    PALETTE_WIDTH = 680., STYLES: 1236 "width: min(680px, 90vw)";
    /// `padding-top: 16vh`, a share of the window's height.
    PALETTE_TOP = 16., STYLES: 1218 "padding-top: 16vh";
    PALETTE_FIELD = 56., STYLES: 1255 "height: 56px";
    PALETTE_FIELD_PAD_LEFT = 18., STYLES: 1256 "padding: 0 var(--space-4) 0 18px";
    PALETTE_ICON = 18., STYLES: 1266 "width: 18px";
    PALETTE_RESULTS_PAD = 6., STYLES: 1306 "padding: 6px";
    PALETTE_ITEM = 40., STYLES: 1319 "height: 40px";
    PALETTE_ITEM_PAD_LEFT = 10., STYLES: 1320 "padding: 0 var(--space-3) 0 10px";
    PALETTE_ITEM_ICON = 24., STYLES: 1332 "width: 24px";
    PALETTE_GO_ARROW = 20., STYLES: 1381 "height: 20px";
    PALETTE_GO_ICON = 12., STYLES: 1387 "width: 12px";
    PALETTE_MODE_PAD_Y = 2., STYLES: 1295 "padding: 2px 8px";
    PALETTE_MODE_PAD_X = 8., STYLES: 1295 "padding: 2px 8px";

    // The page menu the shell draws (shell.css).
    MENU_PAD = 5., SHELL: 78 "padding: 5px";
    MENU_ITEM_PAD_Y = 4., SHELL: 88 "padding: 4px 10px";
    MENU_ITEM_PAD_X = 10., SHELL: 88 "padding: 4px 10px";
    MENU_ITEM_RADIUS = 4., SHELL: 89 "border-radius: 4px";
    MENU_DISABLED_OPACITY = 0.4, SHELL: 118 "opacity: 0.4";
    MENU_MIN_WIDTH = 180., SHELL: 75 "min-width: 180px";
    MENU_SUBMENU_ARROW_GAP = 16., SHELL: 106 "margin-left: 16px";
    MENU_ACCEL_GAP = 24., SHELL: 113 "margin-left: 24px";
    MENU_ACCEL_OPACITY = 0.6, SHELL: 114 "opacity: 0.6";
    MENU_CHECKABLE_PAD_LEFT = 24., SHELL: 122 "padding-left: 24px";
    /// How far the check mark sits left of a checkable item's text.
    MENU_CHECK_OFFSET = 15., SHELL: 128 "margin-left: -15px";
    MENU_SEPARATOR = 1., SHELL: 132 "height: 1px";
    MENU_SEPARATOR_MARGIN_Y = 5., SHELL: 133 "margin: 5px 6px";
    MENU_SEPARATOR_MARGIN_X = 6., SHELL: 133 "margin: 5px 6px";
    /// A menu opened from a button sits this far below it.
    MENU_ANCHOR_GAP = 4., SETTINGS_UI: 202 "y: anchor.bottom + 4";
    /// A menu keeps this far from the window's edges.
    MENU_WINDOW_MARGIN = 4., SETTINGS_UI: 204 "innerWidth - width - 4";
    /// A submenu sits this far right of its item ...
    SUBMENU_GAP = 2., SETTINGS_UI: 92 "const right = at.right + 2";
    /// ... and this far above it, so its first item lines up with the item.
    SUBMENU_RAISE = 5., SETTINGS_UI: 94 "at.top - 5";
    // `.shell-dialog` (shell.css:139-172), the shell's question.
    DIALOG_WIDTH = 360., SHELL: 140 "width: 360px";
    DIALOG_PAD = 20., SHELL: 141 "padding: 20px";
    DIALOG_TEXT_GAP_TOP = 8., SHELL: 159 "margin: 8px 0 18px";
    DIALOG_TEXT_GAP_BOTTOM = 18., SHELL: 159 "margin: 8px 0 18px";
    DIALOG_BUTTON_GAP = 8., SHELL: 171 "gap: 8px";

    // The Space editor (ui/styles.css:1417-1584).
    EDITOR_PAD_TOP = 4., STYLES: 1431 "padding: 4px 14px 12px";
    EDITOR_PAD_X = 14., STYLES: 1431 "padding: 4px 14px 12px";
    EDITOR_PAD_BOTTOM = 12., STYLES: 1431 "padding: 4px 14px 12px";
    FIELD_LABEL_GAP = 4., STYLES: 1451 "margin-top: 4px";
    EMOJI_COLUMNS = 8., STYLES: 1459 "grid-template-columns: repeat(8, 1fr)";
    EMOJI_GAP = 2., STYLES: 1460 "gap: 2px";
    SWATCH_COLUMNS = 7., STYLES: 1493 "grid-template-columns: repeat(7, 1fr)";
    EMOJI_TEXT = 15., STYLES: 1468 "font-size: 15px";
    EMOJI_SELECTED_RING = 1.5, STYLES: 1483 "inset 0 0 0 1.5px var(--selection)";
    EMOJI_ICON = 12., STYLES: 1487 "width: 12px";
    SWATCH_GAP = 6., STYLES: 1494 "gap: 6px";
    SWATCH_SELECTED_GAP = 2., STYLES: 1511 "0 0 0 2px var(--surface)";
    SWATCH_SELECTED_RING = 3.5, STYLES: 1511 "0 0 0 3.5px var(--selection)";
    TOGGLE_ROW_GAP = 6., STYLES: 1533 "margin-top: 6px";
    TOGGLE_ROW_PAD_Y = 10., STYLES: 1534 "padding: 10px 12px";
    TOGGLE_ROW_PAD_X = 12., STYLES: 1534 "padding: 10px 12px";
    TOGGLE_ROW_NOTE_GAP = 1., STYLES: 1546 "margin-top: 1px";

    // Settings.
    SETTINGS_NAV = 216., SETTINGS: 5 "--nav: 216px";
    SETTINGS_NAV_PAD_X = 10., SETTINGS: 57 "padding: 0 10px 16px";
    SETTINGS_NAV_ITEM = 30., SETTINGS: 78 "height: 30px";
    SETTINGS_NAV_ITEM_PAD = 8., SETTINGS: 79 "padding: 0 8px";
    SETTINGS_TITLEBAR = 52., SETTINGS: 150 "height: 52px";
    SETTINGS_PANE_PAD_X = 28., SETTINGS: 151 "padding: 0 28px";
    SETTINGS_ROW_PAD_Y = 10., SETTINGS: 238 "padding: 10px 16px";
    SETTINGS_ROW_PAD_X = 16., SETTINGS: 238 "padding: 10px 16px";
    SETTINGS_CARD_GAP = 14., SETTINGS: 221 "margin-bottom: 14px";
    SETTINGS_BUTTON = 28., SETTINGS: 284 "height: 28px";
    SETTINGS_BUTTON_PAD = 12., SETTINGS: 285 "padding: 0 12px";
    SETTINGS_NAV_GAP = 2., SETTINGS: 56 "gap: 2px";
    SETTINGS_NAV_PAD_BOTTOM = 16., SETTINGS: 57 "padding: 0 10px 16px";
    SETTINGS_NAV_LABEL_TOP = 14., SETTINGS: 67 "margin: 14px 0 4px";
    SETTINGS_NAV_LABEL_BOTTOM = 4., SETTINGS: 67 "margin: 14px 0 4px";
    SETTINGS_NAV_LABEL_PAD = 10., SETTINGS: 68 "padding: 0 10px";
    SETTINGS_NAV_ITEM_GAP = 10., SETTINGS: 77 "gap: 10px";
    SETTINGS_NAV_ICON = 22., SETTINGS: 97 "width: 22px";
    SETTINGS_NAV_ICON_RADIUS = 6., SETTINGS: 99 "border-radius: 6px";
    SETTINGS_NAV_GLYPH = 14., SETTINGS: 105 "width: 14px";
    SETTINGS_PANE_PAD_TOP = 6., SETTINGS: 164 "padding: 6px 28px 28px";
    SETTINGS_PANE_PAD_BOTTOM = 28., SETTINGS: 164 "padding: 6px 28px 28px";
    SETTINGS_SWITCH_WIDTH = 34., SETTINGS: 328 "width: 34px";
    SETTINGS_SWITCH_HEIGHT = 20., SETTINGS: 329 "height: 20px";
    SETTINGS_SWITCH_KNOB_INSET = 2., SETTINGS: 340 "top: 2px";
    SETTINGS_SWITCH_KNOB = 16., SETTINGS: 342 "width: 16px";
    SETTINGS_SWITCH_TRAVEL = 14., SETTINGS: 355 "transform: translateX(14px)";

    // Flechinha.
    FLECHINHA_TOOLBAR = 44., FLECHINHA: 4 "--toolbar: 44px";
    WINDOW_CONTROL = 20., WINDOW_CONTROLS: 9 "width: 20px";
    WINDOW_CONTROL_INSET = 4., WINDOW_CONTROLS: 18 "inset: 4px";
    WINDOW_CONTROL_GLYPH = 10., WINDOW_CONTROLS: 44 "width: 10px";
    /// The room the window controls take at the start of the sidebar's toolbar.
    WINDOW_CONTROLS_WIDTH = 70., STYLES: 97 "width: 70px";
    WINDOW_CONTROLS_PAD_LEFT = 4., STYLES: 98 "padding-left: 4px";
    /// Between the toolbar's lines when back, forward and reload wrap below the controls.
    TOOLBAR_ROW_GAP = 4., STYLES: 117 "row-gap: 4px";
    FLECHINHA_WINDOW_CONTROLS_WIDTH = 72., FLECHINHA: 73 "width: 72px";
    FLECHINHA_WINDOW_CONTROLS_PAD_LEFT = 4., FLECHINHA: 75 "padding-left: 4px";
    FLECHINHA_CTA_PAD_LEFT = 10., FLECHINHA: 329 "padding: 0 12px 0 10px";
    FLECHINHA_CTA_PAD_RIGHT = 12., FLECHINHA: 329 "padding: 0 12px 0 10px";
    FLECHINHA_ADDRESS_MARGIN = 6., FLECHINHA: 122 "margin: 0 6px";
    FLECHINHA_ADDRESS_PAD_RIGHT = 2., FLECHINHA: 123 "padding-right: 2px";
    FLECHINHA_ADDRESS_SHOW_PAD_RIGHT = 4., FLECHINHA: 148 "padding: 0 4px 0 10px";
    FLECHINHA_ADDRESS_SHOW_PAD_LEFT = 10., FLECHINHA: 148 "padding: 0 4px 0 10px";
    FLECHINHA_SITE_ICON = 16., FLECHINHA: 161 "width: 16px";
    FLECHINHA_SITE_ICON_RADIUS = 3., FLECHINHA: 170 "border-radius: 3px";
    FLECHINHA_SITE_GLYPH = 14., FLECHINHA: 174 "width: 14px";
    /// The host's widest share of the address, in percent.
    FLECHINHA_HOST_MAX_WIDTH = 60., FLECHINHA: 184 "max-width: 60%";
    FLECHINHA_EDIT_PAD_RIGHT = 8., FLECHINHA: 215 "padding: 0 8px 0 10px";
    FLECHINHA_ADDRESS_ACTION = 26., FLECHINHA: 244 "width: 26px";
    FLECHINHA_ADDRESS_ACTION_ICON = 15., FLECHINHA: 264 "width: 15px";
    /// The page title's `—` before it, in `--text-3` at this opacity.
    FLECHINHA_TITLE_DASH_GAP = 6., FLECHINHA: 202 "margin-right: 6px";
    FLECHINHA_TITLE_DASH_OPACITY = 0.6, FLECHINHA: 204 "opacity: 0.6";
    FLECHINHA_DISABLED_OPACITY = 0.55, FLECHINHA: 109 "opacity: 0.55";
    /// The loading line along the address's bottom.
    FLECHINHA_LOADING = 2., FLECHINHA: 278 "height: 2px";
    /// Its share of the address, in percent, how far right it travels in percent of its own
    /// width, and how long one pass takes.
    FLECHINHA_LOADING_WIDTH = 30., FLECHINHA: 279 "width: 30%";
    FLECHINHA_LOADING_TRAVEL = 340., FLECHINHA: 291 "translateX(340%)";
    FLECHINHA_LOADING_SECONDS = 1.1, FLECHINHA: 282 "loading 1.1s";

    // The environment badge, the agent's pill and folder, and "Running locally".
    ENV_BADGE_PAD_Y = 1., STYLES: 302 "padding: 1px 6px";
    ENV_BADGE_PAD_X = 6., STYLES: 302 "padding: 1px 6px";
    ENV_BADGE_RADIUS = 4., STYLES: 303 "border-radius: 4px";
    ENV_BADGE_TEXT = 10., STYLES: 304 "font: 700 10px / 16px var(--mono)";
    ENV_BADGE_LINE = 16., STYLES: 304 "font: 700 10px / 16px var(--mono)";
    AGENT_STATUS = 30., STYLES: 371 "height: 30px";
    AGENT_STATUS_PAD = 10., STYLES: 372 "padding: 0 10px";
    /// The agent's colour at this percentage behind its pill.
    AGENT_STATUS_SHARE = 13., STYLES: 374 "color-mix(in srgb, var(--agent) 13%, transparent)";
    AGENT_DOT = 7., STYLES: 392 "width: 7px";
    /// The agent folder's guide in its colour at this percentage.
    AGENT_GUIDE_SHARE = 40., STYLES: 845 "color-mix(in srgb, var(--agent) 40%, transparent)";
    LOCAL_SERVER_ROW = 28., STYLES: 894 "height: 28px";
    LIVE_DOT = 7., STYLES: 900 "width: 7px";
    LIVE_DOT_MARGIN = 4., STYLES: 902 "margin: 0 4px";
    LIVE_DOT_RING = 3., STYLES: 905 "box-shadow: 0 0 0 3px color-mix(in srgb, var(--green) 20%, transparent)";
    LIVE_DOT_RING_SHARE = 20., STYLES: 905 "color-mix(in srgb, var(--green) 20%, transparent)";

    // The empty page (ui/styles.css:1629-1820).
    /// The Space's colour under this percentage of `--surface`.
    EMPTY_SURFACE_SHARE = 72., STYLES: 1639 "color-mix(in srgb, var(--surface) 72%";
    EMPTY_HAIRLINE = 0.5, STYLES: 1640 "box-shadow: 0 0 0 0.5px var(--line), var(--shadow-row)";
    EMPTY_BODY = 560., STYLES: 1655 "width: min(560px, 100% - 48px)";
    EMPTY_BODY_MARGIN = 48., STYLES: 1655 "width: min(560px, 100% - 48px)";
    EMPTY_BODY_PAD_TOP = 48., STYLES: 1656 "padding: 48px 0 24px";
    EMPTY_BODY_PAD_BOTTOM = 24., STYLES: 1656 "padding: 48px 0 24px";
    EMPTY_SPACE_GAP = 6., STYLES: 1662 "gap: 6px";
    EMPTY_SPACE_PAD_Y = 4., STYLES: 1663 "padding: 4px 10px";
    EMPTY_SPACE_PAD_X = 10., STYLES: 1663 "padding: 4px 10px";
    EMPTY_SPACE_EMOJI = 13., STYLES: 1673 "font-size: 13px";
    EMPTY_TITLE_TOP = 14., STYLES: 1677 "margin: 14px 0 22px";
    EMPTY_TITLE_BOTTOM = 22., STYLES: 1677 "margin: 14px 0 22px";
    EMPTY_TITLE = 28., STYLES: 1678 "font-size: 28px";
    EMPTY_TITLE_WEIGHT = 650., STYLES: 1679 "font-weight: 650";
    EMPTY_SEARCH_GAP = 10., STYLES: 1686 "gap: 10px";
    EMPTY_SEARCH = 48., STYLES: 1688 "height: 48px";
    EMPTY_SEARCH_PAD_RIGHT = 12., STYLES: 1689 "padding: 0 12px 0 16px";
    EMPTY_SEARCH_PAD_LEFT = 16., STYLES: 1689 "padding: 0 12px 0 16px";
    EMPTY_SEARCH_RADIUS = 14., STYLES: 1690 "border-radius: 14px";
    EMPTY_SEARCH_ICON = 18., STYLES: 1708 "width: 18px";
    EMPTY_SECTION_TOP = 32., STYLES: 1714 "margin-top: 32px";
    EMPTY_HEADER_PAD = 4., STYLES: 1722 "margin: 0 4px 10px";
    EMPTY_HEADER_BOTTOM = 10., STYLES: 1722 "margin: 0 4px 10px";
    EMPTY_HEADER_WEIGHT = 600., STYLES: 1724 "font-weight: 600";
    TOP_SITE_COLUMNS = 4., STYLES: 1732 "grid-template-columns: repeat(4, 1fr)";
    TOP_SITES_GAP = 8., STYLES: 1733 "gap: 8px";
    TOP_SITE_GAP = 8., STYLES: 1740 "gap: 8px";
    TOP_SITE_PAD_TOP = 14., STYLES: 1742 "padding: 14px 8px 10px";
    TOP_SITE_PAD_X = 8., STYLES: 1742 "padding: 14px 8px 10px";
    TOP_SITE_PAD_BOTTOM = 10., STYLES: 1742 "padding: 14px 8px 10px";
    TOP_SITE_ICON = 44., STYLES: 1754 "width: 44px";
    TOP_SITE_ICON_RADIUS = 12., STYLES: 1756 "border-radius: 12px";
    TOP_SITE_FAVICON = 22., STYLES: 1762 "width: 22px";
    TOP_SITE_FAVICON_RADIUS = 5., STYLES: 1764 "border-radius: 5px";
    TOP_SITE_LETTER = 12., STYLES: 1765 "font-size: 12px";
    LOCAL_CHIPS_GAP = 8., STYLES: 1780 "gap: 8px";
    LOCAL_CHIP_GAP = 8., STYLES: 1786 "gap: 8px";
    LOCAL_CHIP = 32., STYLES: 1787 "height: 32px";
    LOCAL_CHIP_PAD = 12., STYLES: 1788 "padding: 0 12px";
    EMPTY_HINTS_GAP_Y = 6., STYLES: 1810 "gap: 6px 18px";
    EMPTY_HINTS_GAP_X = 18., STYLES: 1810 "gap: 6px 18px";
    EMPTY_HINTS_PAD_TOP = 16., STYLES: 1811 "padding: 16px 24px 20px";
    EMPTY_HINTS_PAD_X = 24., STYLES: 1811 "padding: 16px 24px 20px";
    EMPTY_HINTS_PAD_BOTTOM = 20., STYLES: 1811 "padding: 16px 24px 20px";
    EMPTY_HINT_GAP = 5., STYLES: 1819 "gap: 5px";

    // Settings' search (ui/settings.css:1326-1420) and its lead paragraphs.
    NAV_SEARCH_GAP = 6., SETTINGS: 1331 "gap: 6px";
    NAV_SEARCH = 28., SETTINGS: 1332 "height: 28px";
    NAV_SEARCH_BOTTOM = 10., SETTINGS: 1333 "margin: 0 0 10px";
    NAV_SEARCH_PAD = 8., SETTINGS: 1334 "padding: 0 8px";
    NAV_SEARCH_ICON = 13., SETTINGS: 1346 "width: 13px";
    NAV_NO_MATCH_OPACITY = 0.35, SETTINGS: 1365 "opacity: 0.35";
    SEARCH_RESULTS_GAP = 2., SETTINGS: 1371 "gap: 2px";
    RESULT_GAP = 12., SETTINGS: 1381 "gap: 12px";
    RESULT_PAD_Y = 8., SETTINGS: 1382 "padding: 8px 12px";
    RESULT_PAD_X = 12., SETTINGS: 1382 "padding: 8px 12px";
    NO_RESULTS_MARGIN = 40., SETTINGS: 1408 "margin: 40px 0";
    LEAD_BOTTOM = 16., SETTINGS: 173 "margin: 0 0 16px";
    LEAD_LINE_HEIGHT = 1.5, SETTINGS: 175 "line-height: 1.5";
    MONO_SHARE = 0.92, SETTINGS: 188 "font-size: 0.92em";
    SETTINGS_BUTTON_DISABLED_OPACITY = 0.45, SETTINGS: 303 "opacity: 0.45";

    // The page area the browser frame lays out (chromium/patches).
    PAGE_GAP = 8., FRAME_API: 69 "kFlechaGap = 8";
    SPLIT_GAP = 8., SPLIT: 213 "kFlechaSplitGap = 8";
    WINDOW_RADIUS = 20., TOKENS: 85 "--window-radius: 20px";
}

/// `--page-radius: calc(var(--window-radius) - 8px)` (ui/tokens.css:86): the page area's
/// corners, concentric with the window's.
pub(crate) const PAGE_RADIUS: f32 = WINDOW_RADIUS - PAGE_GAP;

/// The page area's drop shadow in the gap around it (patch 0041, lines 36-43): Flecha's ink in
/// Light, black on a dark Space colour.
pub(crate) fn page_shadow(dark: bool) -> Vec<BoxShadow> {
    let ink = if dark { [0., 0., 0.] } else { [11., 37., 69.] };
    let [close, wide] = if dark { [0x59, 0x40] } else { [0x1a, 0x0f] };
    [(1., 3., close), (4., 14., wide)]
        .into_iter()
        .map(|(y, blur, alpha): (f32, f32, u8)| BoxShadow {
            color: Rgba {
                r: ink[0] / 255.,
                g: ink[1] / 255.,
                b: ink[2] / 255.,
                a: f32::from(alpha) / 255.,
            }
            .into(),
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.),
            inset: false,
        })
        .collect()
}

/// The first family of `--mono` (ui/tokens.css:66), which ships with the shell.
pub(crate) const MONO: &str = "JetBrains Mono";

/// The named families of `--font` (ui/tokens.css:65), in order. Its `-apple-system` and
/// `BlinkMacSystemFont` keywords and the generic `sans-serif` have no family name GPUI can load.
pub(crate) const FONT: &[&str] = &["SF Pro Text", "Inter"];

/// Chromium's Linux `sans-serif` family (chrome/app/resources/locale_settings_linux.grd:169-171,
/// Chromium 158), which fontconfig then substitutes with an installed face.
pub(crate) const LINUX_SANS_SERIF: &str = "Arial";

/// The `--ease` curve, `cubic-bezier(0.2, 0.8, 0.2, 1)` (ui/tokens.css:90).
pub(crate) const EASE: (f32, f32, f32, f32) = (0.2, 0.8, 0.2, 1.);

/// `--medium`, for panels that open.
pub(crate) const MEDIUM: Duration = Duration::from_millis(MEDIUM_MS as u64);
pub(crate) const SPINNER_TURN: Duration = Duration::from_millis(SPINNER_MS as u64);

/// `--ease` as a GPUI easing: the progress along the curve at `t` of its duration.
pub(crate) fn ease(t: f32) -> f32 {
    cubic_bezier(EASE, t)
}

/// The easing of the loading line's long second stretch (ui/app.ts:335).
const PROGRESS_EASE: (f32, f32, f32, f32) = (0.2, 0.6, 0.35, 1.);

/// How far along the address the loading line is, as a share of its width, at `t` of
/// `PROGRESS_MS` (the `PROGRESS` keyframes, ui/app.ts:333-337): linearly to 30% in the first 3%,
/// then slowing towards 90%.
pub(crate) fn progress(t: f32) -> f32 {
    let (first, last) = (PROGRESS_FIRST / 100., PROGRESS_LAST / 100.);
    if t < PROGRESS_FIRST_AT {
        return first * t.max(0.) / PROGRESS_FIRST_AT;
    }
    let rest = (t - PROGRESS_FIRST_AT) / (1. - PROGRESS_FIRST_AT);
    first + (last - first) * cubic_bezier(PROGRESS_EASE, rest)
}

fn cubic_bezier((x1, y1, x2, y2): (f32, f32, f32, f32), t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let curve = |a: f32, b: f32, s: f32| {
        let inv = 1. - s;
        3. * inv * inv * s * a + 3. * inv * s * s * b + s * s * s
    };
    // Bisection on x, which the curve's control points keep monotonic. Each step halves the
    // interval, so 24 steps reach f32's precision on [0, 1].
    let (mut low, mut high) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if curve(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    curve(y1, y2, (low + high) / 2.)
}

/// The value of a token in Light or Dark, with `var()` references to other colour tokens resolved.
pub(crate) fn css(table: &[Css], name: &str, dark: bool) -> &'static str {
    let entry = table
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is not a cited token"));
    let value = match (dark, entry.dark) {
        (true, Some((_, value))) => value,
        _ => entry.light.1,
    };
    match value
        .strip_prefix("var(")
        .and_then(|value| value.strip_suffix(')'))
    {
        Some(reference) => css(COLORS, reference, dark),
        None => value,
    }
}

pub(crate) fn color(name: &str, dark: bool) -> Rgba {
    let value = css(COLORS, name, dark);
    parse_color(value).unwrap_or_else(|| panic!("{name}: {value} is not a colour"))
}

/// A CSS colour as the shipping tokens write them: `#rgb`, `#rrggbb` or `rgba(r, g, b, a)`.
pub(crate) fn parse_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        let [r, g, b] = match digits[..] {
            [r, g, b] => [r * 17, g * 17, b * 17],
            [r1, r2, g1, g2, b1, b2] => [r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2],
            _ => return None,
        };
        return Some(Rgba {
            r: f32::from(r) / 255.,
            g: f32::from(g) / 255.,
            b: f32::from(b) / 255.,
            a: 1.,
        });
    }
    let channels = value.strip_prefix("rgba(")?.strip_suffix(')')?;
    let channels: Vec<f32> = channels
        .split(',')
        .map(|c| c.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [r, g, b, a] = channels[..] else {
        return None;
    };
    Some(Rgba {
        r: r / 255.,
        g: g / 255.,
        b: b / 255.,
        a,
    })
}

/// A `box-shadow` list: offsets, blur and spread in px, then a colour or a `var()` of a colour
/// token, read in Light or Dark.
pub(crate) fn parse_shadow(value: &str, dark: bool) -> Option<Vec<BoxShadow>> {
    let mut layers = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut split = Vec::new();
    for (index, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                split.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    split.push(&value[start..]);
    for layer in split {
        let layer = layer.trim();
        let (inset, layer) = match layer.strip_prefix("inset ") {
            Some(layer) => (true, layer),
            None => (false, layer),
        };
        let color_at = layer
            .find("rgba(")
            .or_else(|| layer.find('#'))
            .or_else(|| layer.find("var("))?;
        let lengths: Vec<f32> = layer[..color_at]
            .split_whitespace()
            .map(|length| length.trim_end_matches("px").parse().ok())
            .collect::<Option<_>>()?;
        let (x, y, blur, spread) = match lengths[..] {
            [x, y] => (x, y, 0., 0.),
            [x, y, blur] => (x, y, blur, 0.),
            [x, y, blur, spread] => (x, y, blur, spread),
            _ => return None,
        };
        layers.push(BoxShadow {
            color: match layer[color_at..]
                .strip_prefix("var(")
                .and_then(|name| name.strip_suffix(')'))
            {
                Some(name) => parse_color(css(COLORS, name, dark))?,
                None => parse_color(&layer[color_at..])?,
            }
            .into(),
            offset: point(px(x), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
            inset,
        });
    }
    Some(layers)
}

static SHADOW_CACHE: LazyLock<[[Vec<BoxShadow>; 2]; 9]> = LazyLock::new(|| {
    let both = |name| {
        [false, true].map(|dark| {
            let value = css(SHADOWS, name, dark);
            parse_shadow(value, dark).unwrap_or_else(|| panic!("{name}: {value} is not a shadow"))
        })
    };
    [
        both("--shadow-row"),
        both("--shadow-pop"),
        both(".switch::after"),
        both(".window-control::before"),
        both(".card"),
        both(".button"),
        both(".nav-icon"),
        both("main"),
        both("#address.editing"),
    ]
});

/// A Flechinha address being edited: the focus ring and a slight lift.
pub(crate) fn shadow_address_editing(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[8][usize::from(dark)].clone()
}

/// `--shadow-row`: the selected row's lift.
pub(crate) fn shadow_row(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[0][usize::from(dark)].clone()
}

/// `--shadow-pop`: floating surfaces (the command bar, menus, popovers).
pub(crate) fn shadow_pop(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[1][usize::from(dark)].clone()
}

/// The switch knob's shadow.
pub(crate) fn shadow_knob() -> Vec<BoxShadow> {
    SHADOW_CACHE[2][0].clone()
}

/// The ring inside a window control's circle.
pub(crate) fn shadow_window_control() -> Vec<BoxShadow> {
    SHADOW_CACHE[3][0].clone()
}

/// A Settings card's hairline and lift.
pub(crate) fn shadow_card(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[4][usize::from(dark)].clone()
}

/// A Settings `.button`'s hairline and lift.
pub(crate) fn shadow_button(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[5][usize::from(dark)].clone()
}

/// The ring inside a Settings pane icon.
pub(crate) fn shadow_nav_icon() -> Vec<BoxShadow> {
    SHADOW_CACHE[6][0].clone()
}

/// The hairline down the left of the Settings pane, beside its list of panes.
pub(crate) fn shadow_pane_edge(dark: bool) -> Vec<BoxShadow> {
    SHADOW_CACHE[7][usize::from(dark)].clone()
}

/// CSS `color-mix(in srgb, a p, b)`: interpolates premultiplied channels, as the spec says.
pub(crate) fn mix(a: Rgba, share: f32, b: Rgba) -> Rgba {
    let alpha = a.a * share + b.a * (1. - share);
    if alpha == 0. {
        return Rgba::default();
    }
    let channel = |x: f32, y: f32| (x * a.a * share + y * b.a * (1. - share)) / alpha;
    Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
}

/// `spaceTint` (lib/spaceTint.ts): a Space's pastel in Light, a deep tint of its hue in Dark.
pub(crate) fn space_tint(color: Rgba, dark: bool) -> Rgba {
    if !dark {
        return color;
    }
    // lib/spaceTint.ts:4-6: 16% of the colour over [17, 24, 36], rounded to whole channels.
    let base = [17., 24., 36.];
    let channel = |c: f32, base: f32| ((c * 255.) * 0.16 + base * 0.84).round() / 255.;
    Rgba {
        r: channel(color.r, base[0]),
        g: channel(color.g, base[1]),
        b: channel(color.b, base[2]),
        a: 1.,
    }
}

/// `hueFor` (ui/app.ts:94-98): a stable hue per site for its letter placeholder.
pub(crate) fn hue_for(text: &str) -> u32 {
    let mut hash: u32 = 0;
    // `for (const char of text)` walks code points; `charCodeAt(0)` is each one's first unit.
    for c in text.chars() {
        let unit = c.encode_utf16(&mut [0; 2])[0];
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    hash % 360
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The shipping files the citations point into, read from the checkout.
    pub(crate) fn source(file: &str) -> &'static str {
        match file {
            "ui/tokens.css" => include_str!("../../../../chromium/shell/ui/tokens.css"),
            "ui/styles.css" => include_str!("../../../../chromium/shell/ui/styles.css"),
            "ui/settings.css" => include_str!("../../../../chromium/shell/ui/settings.css"),
            "ui/flechinha.css" => include_str!("../../../../chromium/shell/ui/flechinha.css"),
            "shell.css" => include_str!("../../../../chromium/shell/shell.css"),
            "ui/icons.ts" => include_str!("../../../../chromium/shell/ui/icons.ts"),
            "ui/windowControls.css" => {
                include_str!("../../../../chromium/shell/ui/windowControls.css")
            }
            "ui/flechinha.html" => include_str!("../../../../chromium/shell/ui/flechinha.html"),
            "ui/index.html" => include_str!("../../../../chromium/shell/ui/index.html"),
            "settingsUi.ts" => include_str!("../../../../chromium/shell/settingsUi.ts"),
            "ui/spaceChoices.ts" => include_str!("../../../../chromium/shell/ui/spaceChoices.ts"),
            "lib/spaceTint.ts" => include_str!("../../../../chromium/shell/lib/spaceTint.ts"),
            "ui/app.ts" => include_str!("../../../../chromium/shell/ui/app.ts"),
            "../patches/0010-flecha-private-extension-api.patch" => {
                include_str!("../../../../chromium/patches/0010-flecha-private-extension-api.patch")
            }
            "../patches/0019-flat-split-view-and-split-ratio.patch" => include_str!(
                "../../../../chromium/patches/0019-flat-split-view-and-split-ratio.patch"
            ),
            "../patches/0023-flechinha-windows.patch" => {
                include_str!("../../../../chromium/patches/0023-flechinha-windows.patch")
            }
            "../patches/0041-page-area-shadow.patch" => {
                include_str!("../../../../chromium/patches/0041-page-area-shadow.patch")
            }
            other => panic!("no shipping source {other}"),
        }
    }

    fn line(file: &str, number: u32) -> &'static str {
        source(file)
            .lines()
            .nth(number as usize - 1)
            .unwrap_or_else(|| panic!("{file} has no line {number}"))
    }

    fn assert_cited(table: &[Css]) {
        for entry in table {
            for (number, value) in [Some(entry.light), entry.dark].into_iter().flatten() {
                let text = line(entry.file, number);
                let expected = if entry.name.starts_with("--") {
                    format!("{}: {value};", entry.name)
                } else {
                    value.to_owned()
                };
                assert!(
                    text.contains(&expected),
                    "{}:{number} is `{}`, not `{expected}`",
                    entry.file,
                    text.trim()
                );
            }
        }
    }

    #[test]
    fn every_colour_and_shadow_matches_its_shipping_line() {
        assert_cited(COLORS);
        assert_cited(SHADOWS);
    }

    #[test]
    fn every_dark_override_sits_in_the_dark_media_block() {
        for entry in COLORS.iter().chain(SHADOWS) {
            let Some((dark, _)) = entry.dark else {
                continue;
            };
            let opened = source(entry.file)
                .lines()
                .take(dark as usize)
                .filter(|line| line.contains("@media (prefers-color-scheme: dark)"))
                .count();
            assert!(
                opened > 0,
                "{} line {dark} is not in a dark block",
                entry.name
            );
            assert!(entry.light.0 < dark, "{}", entry.name);
        }
    }

    #[test]
    fn every_token_parses_in_both_themes() {
        for entry in COLORS {
            for dark in [false, true] {
                color(entry.name, dark);
            }
        }
        for dark in [false, true] {
            assert_eq!(shadow_row(dark).len(), if dark { 1 } else { 2 });
            assert_eq!(shadow_pop(dark).len(), 2);
            // The card's hairline is `--card-line` in each theme.
            let card = shadow_card(dark);
            assert_eq!(card.len(), 2);
            assert_eq!(card[0].color, color("--card-line", dark).into());
            assert_eq!(
                shadow_button(dark)[0].color,
                color("--surface-line", dark).into()
            );
        }
        assert!(shadow_nav_icon()[0].inset);
        assert_eq!(color("--cta", false), color("--ink", false));
    }

    #[test]
    fn every_metric_matches_its_shipping_declaration() {
        for metric in METRICS {
            let text = line(metric.file, metric.line);
            assert!(
                text.contains(metric.declaration),
                "{}: {}:{} is `{}`, not `{}`",
                metric.name,
                metric.file,
                metric.line,
                text.trim(),
                metric.declaration
            );
            // TypeScript numbers may group digits with `_` (`10_000`).
            let numbers: Vec<f32> = metric
                .declaration
                .replace('_', "")
                .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                .filter_map(|number| number.parse().ok())
                .collect();
            assert!(
                numbers.contains(&metric.value),
                "{} is {}, not a value of `{}`",
                metric.name,
                metric.value,
                metric.declaration
            );
        }
    }

    #[test]
    fn page_geometry_follows_the_frame_patches() {
        assert!(line("ui/tokens.css", 86).contains("calc(var(--window-radius) - 8px)"));
        assert!(line("ui/tokens.css", 66).contains(&format!("--mono: '{MONO}',")));
        let font = FONT
            .iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            line("ui/tokens.css", 65).contains(&format!("BlinkMacSystemFont, {font}, sans-serif"))
        );
        assert_eq!(PAGE_RADIUS, 12.);
        let shadow = source("../patches/0041-page-area-shadow.patch");
        for expected in [
            "SkColorSetRGB(0x0b, 0x25, 0x45)",
            "gfx::ShadowValue(gfx::Vector2d(0, 1), 3,",
            "SkColorSetA(ink, dark ? 0x59 : 0x1a)",
            "gfx::ShadowValue(gfx::Vector2d(0, 4), 14,",
            "SkColorSetA(ink, dark ? 0x40 : 0x0f)",
        ] {
            assert!(shadow.contains(expected), "{expected}");
        }
        let light = page_shadow(false);
        assert_eq!(light[1].blur_radius, px(14.));
        assert_eq!(light[0].offset, point(px(0.), px(1.)));
        let flechinha = source("../patches/0023-flechinha-windows.patch");
        assert!(
            flechinha
                .contains("gfx::Insets::TLBR(kFlechinhaToolbarHeight, kFlechaGap, kFlechaGap,")
        );
    }

    #[test]
    fn ease_follows_the_shipping_curve() {
        assert!(line("ui/tokens.css", 90).contains("--ease: cubic-bezier(0.2, 0.8, 0.2, 1);"));
        assert!(ease(0.).abs() < 1e-5);
        assert!((ease(1.) - 1.).abs() < 1e-5);
        // The curve rises fast and settles: ahead of linear throughout.
        for step in 1..10 {
            let t = step as f32 / 10.;
            assert!(ease(t) > t, "{t}");
        }
        assert!(ease(0.5) > 0.85);
    }

    #[test]
    fn the_loading_line_runs_the_shipping_keyframes() {
        assert!(line("ui/app.ts", 335).contains("easing: 'cubic-bezier(0.2, 0.6, 0.35, 1)'"));
        assert_eq!(progress(0.), 0.);
        assert!((progress(PROGRESS_FIRST_AT) - 0.3).abs() < 1e-5);
        assert!((progress(1.) - 0.9).abs() < 1e-5);
        let mut last = 0.;
        for step in 1..=100 {
            let now = progress(step as f32 / 100.);
            assert!(now >= last, "{step}");
            last = now;
        }
    }

    #[test]
    fn space_tint_matches_the_shipping_function() {
        let shipping = source("lib/spaceTint.ts");
        assert!(shipping.contains("const base = [17, 24, 36];"));
        assert!(shipping.contains("Math.round(c * 0.16 + base[i] * 0.84)"));
        let tint = |hex: &str, dark| {
            let Rgba { r, g, b, .. } = space_tint(parse_color(hex).unwrap(), dark);
            [r, g, b].map(|c| (c * 255.).round() as u8)
        };
        // 205*0.16+17*0.84 = 47.08, 217*0.16+24*0.84 = 54.88, 247*0.16+36*0.84 = 69.76.
        assert_eq!(tint("#cdd9f7", true), [47, 55, 70]);
        assert_eq!(tint("#cdd9f7", false), [0xcd, 0xd9, 0xf7]);
        assert_eq!(tint("#ddd5f3", true), [50, 54, 69]);
    }

    #[test]
    fn mix_matches_color_mix() {
        let agent = color("--agent", false);
        let faded = mix(agent, 0.13, Rgba::default());
        assert!((faded.a - 0.13).abs() < 1e-6);
        assert!((faded.r - agent.r).abs() < 1e-6);
        let white = parse_color("#fff").unwrap();
        let black = parse_color("#000").unwrap();
        assert!((mix(white, 0.72, black).r - 0.72).abs() < 1e-6);
    }

    #[test]
    fn hue_for_matches_app_ts() {
        assert!(source("ui/tokens.css").contains("--ease"));
        // (hash * 31 + charCode) >>> 0, per character: "a" is 97, "ab" is 97*31+98 = 3105.
        assert_eq!(hue_for("a"), 97);
        assert_eq!(hue_for("ab"), 3105 % 360);
        assert_eq!(hue_for(""), 0);
    }
}
