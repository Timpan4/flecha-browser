//! The shipping shell's one icon set (chromium/shell/ui/icons.ts), drawn by GPUI from the same
//! path data. Each icon is an SVG on a 16px grid with 1.5px round strokes, as the `svg` rule in
//! ui/tokens.css draws it; GPUI paints the stroke in the element's text colour.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, Hsla, SharedString, Styled as _, Svg, px, svg};

use crate::tokens::ICON_STROKE;

/// Each icon's name and SVG path `d`, in icons.ts order.
pub(crate) const ICONS: &[(&str, &str)] = &[
    ("close", "M4.5 4.5l7 7M11.5 4.5l-7 7"),
    ("plus", "M8 3.5v9M3.5 8h9"),
    ("chevron", "M6.5 4.5 10 8l-3.5 3.5"),
    (
        "folder",
        "M2 4.5A1.5 1.5 0 0 1 3.5 3h2.6l1.4 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z",
    ),
    (
        "folderOpen",
        "M2 11.5V4.5A1.5 1.5 0 0 1 3.5 3h2.6l1.4 1.5h4A1.5 1.5 0 0 1 13 6v.5M2 11.5l1.6-4A1.5 1.5 0 0 1 5 6.5h8.6a.9.9 0 0 1 .8 1.2l-1.5 4.3a1.5 1.5 0 0 1-1.4 1h-8A1.5 1.5 0 0 1 2 11.5z",
    ),
    (
        "lock",
        "M4.5 7.5h7a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1zM5.5 7.5V5.5a2.5 2.5 0 0 1 5 0v2",
    ),
    (
        "unlocked",
        "M4.5 7.5h7a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1zM5.5 7.5V5.5a2.5 2.5 0 0 1 4.8-1",
    ),
    ("code", "M5.5 5 2.5 8l3 3M10.5 5l3 3-3 3"),
    (
        "search",
        "M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10zM10.6 10.6 14 14",
    ),
    (
        "globe",
        "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12zM2 8h12M8 2c1.6 1.7 2.4 3.7 2.4 6S9.6 12.3 8 14c-1.6-1.7-2.4-3.7-2.4-6S6.4 3.7 8 2z",
    ),
    (
        "tab",
        "M2.5 4.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1zM2.5 6.5h11",
    ),
    (
        "history",
        "M2.5 8a5.5 5.5 0 1 0 1.6-3.9M2.5 2.5v2.6h2.6M8 5v3l2 1.5",
    ),
    ("archive", "M2.5 3.5h11v3h-11zM3.5 6.5v6h9v-6M6.5 9h3"),
    (
        "phone",
        "M5 1.5h6a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1zM7 12.5h2",
    ),
    ("columns", "M2.5 3.5h11v9h-11zM8 3.5v9"),
    ("zap", "M9 1.5 3.5 9H8l-1 5.5L12.5 7H8z"),
    (
        "settings",
        "M8 10a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM8 1.5v2M8 12.5v2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M1.5 8h2M12.5 8h2M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4",
    ),
    (
        "sparkle",
        "M8 1.5l1.4 4.1L13.5 7l-4.1 1.4L8 12.5 6.6 8.4 2.5 7l4.1-1.4zM12.5 11.5l.6 1.4 1.4.6-1.4.6-.6 1.4-.6-1.4-1.4-.6 1.4-.6z",
    ),
    ("copy", "M5.5 5.5h7v8h-7zM3.5 10.5v-8h7"),
    ("external", "M6.5 3.5h-3v9h9v-3M9 3h4v4M13 3 7.5 8.5"),
    ("download", "M8 2.5v8m-3-3 3 3 3-3M3 13.5h10"),
    ("space", "M2.5 3.5h11v9h-11zM5.5 3.5v9"),
    (
        "import",
        "M8 2.5v7m-3-3 3 3 3-3M2.5 10.5v2a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1v-2",
    ),
    (
        "key",
        "M5.5 10.5a3 3 0 1 1 2.6-1.5L14 14.5M11 11.5l1.5-1.5M12.5 13l1.5-1.5",
    ),
    (
        "clock",
        "M8 14A6 6 0 1 0 8 2a6 6 0 0 0 0 12zM8 4.5V8l2.5 1.5",
    ),
    ("sidebar", "M2.5 3.5h11v9h-11zM6 3.5v9"),
    ("arrowLeft", "M9.5 3.5 5 8l4.5 4.5"),
    ("arrowRight", "M6.5 3.5 11 8l-4.5 4.5"),
    ("reload", "M13 8a5 5 0 1 1-1.46-3.54M13 2.5v3h-3"),
    (
        "terminal",
        "M2.5 3.5h11v9h-11zM5 6.5 7 8.5 5 10.5M8.5 10.5h3",
    ),
    (
        "link",
        "M6.8 9.2a2.5 2.5 0 0 0 3.6 0l2.3-2.3a2.5 2.5 0 0 0-3.6-3.6l-.8.8M9.2 6.8a2.5 2.5 0 0 0-3.6 0L3.3 9.1a2.5 2.5 0 0 0 3.6 3.6l.8-.8",
    ),
    (
        "sliders",
        "M2.5 5h6M11.5 5h2M2.5 11h2M7.5 11h6M10 3.5v3M6 9.5v3",
    ),
    (
        "camera",
        "M2.5 5.5a1 1 0 0 1 1-1h1.8l1-1.5h3.4l1 1.5h1.8a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1zM8 10.5a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
    ),
    (
        "wrench",
        "M10.5 2.5a3 3 0 0 0-2.8 4L3 11.2a1.2 1.2 0 0 0 1.8 1.8l4.7-4.7a3 3 0 0 0 4-2.8l-1.8 1-1.4-1.4 1-1.8z",
    ),
    ("minus", "M3.5 8h9"),
    (
        "palette",
        "M8 14a6 6 0 1 1 6-6c0 1.7-1.3 2.5-2.6 2.5H10a1.2 1.2 0 0 0-.9 2A1 1 0 0 1 8 14zM5 8h.01M6.5 5h.01M9.5 5h.01M11 8h.01",
    ),
    (
        "keyboard",
        "M2 4.5h12v7H2zM4.5 7h.01M6.5 7h.01M8.5 7h.01M10.5 7h.01M5 9.5h6",
    ),
    (
        "volume",
        "M2.5 6.5h2.5L8 4v8L5 9.5H2.5zM10.5 6a3 3 0 0 1 0 4M12 4.5a5 5 0 0 1 0 7",
    ),
    (
        "muted",
        "M2.5 6.5h2.5L8 4v8L5 9.5H2.5zM10.5 6.5l3 3M13.5 6.5l-3 3",
    ),
    ("miniWindow", "M2.5 3.5h11v9h-11zM2.5 6h11M8.5 8.5h3v2h-3z"),
    (
        "trash",
        "M3 4.5h10M6.5 4.5v-1h3v1M4.5 4.5l.6 8a1 1 0 0 0 1 .9h3.8a1 1 0 0 0 1-.9l.6-8",
    ),
    ("check", "M3 8.5 6.5 12 13 4.5"),
    (
        "shield",
        "M8 1.5 13 3.5v4c0 3-2.2 5.3-5 7-2.8-1.7-5-4-5-7v-4z",
    ),
    (
        "stack",
        "M3.5 7h9a1 1 0 0 1 1 1v4.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1zM4.5 4.75h7M6 2.5h4",
    ),
    (
        "star",
        "M8 2.2l1.75 3.6 3.95.55-2.85 2.8.7 3.95L8 11.2l-3.55 1.9.7-3.95-2.85-2.8 3.95-.55z",
    ),
    (
        "pencil",
        "M10.2 3.3a1.4 1.4 0 0 1 2 0l.5.5a1.4 1.4 0 0 1 0 2L6 12.5l-3 .5.5-3zM9 4.5l2.5 2.5",
    ),
    (
        "folderPlus",
        "M2 4.5A1.5 1.5 0 0 1 3.5 3h2.6l1.4 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5zM8 7v4M6 9h4",
    ),
    ("menu", "M2.5 4.5h11M2.5 8h11M2.5 11.5h11"),
    (
        "puzzle",
        "M2.75 4.75h2.5a1.5 1.5 0 1 1 3 0h2.5v2.5a1.5 1.5 0 1 1 0 3v2.5h-8z",
    ),
];

/// The close circle's cross (ui/flechinha.html:18 and ui/index.html:21): a 12px grid with
/// 1.4px strokes (ui/windowControls.css:44-48), served as `icons/window-close.svg`.
pub(crate) const WINDOW_CLOSE: &str = "M3.75 3.75l4.5 4.5M8.25 3.75l-4.5 4.5";

/// The loading spinner's asset path. `.favicon.spinner` (ui/styles.css:701-709) draws it as a
/// 14px box with a 2px border, 1px in from the favicon's 16px: on the 16px grid, a ring of
/// radius 6 and width 2 at a quarter strength, and its top border at full strength. A border's
/// top side meets the others at 45 degrees, so the strong arc runs from 225 to 315 degrees.
pub(crate) const SPINNER: &str = "icons/spinner.svg";

const SPINNER_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="2"><circle cx="8" cy="8" r="6" stroke-opacity="0.25"/><path d="M3.757 3.757A6 6 0 0 1 12.243 3.757"/></svg>"##;

/// Serves `icons/<name>.svg` for each icon in [`ICONS`], the window close cross and the
/// loading spinner.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path
            .strip_prefix("icons/")
            .and_then(|name| name.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        if name == "window-close" {
            return Ok(Some(Cow::Owned(window_control(WINDOW_CLOSE).into_bytes())));
        }
        if name == "spinner" {
            return Ok(Some(Cow::Borrowed(SPINNER_SVG.as_bytes())));
        }
        Ok(ICONS
            .iter()
            .find(|(icon, _)| *icon == name)
            .map(|(_, d)| Cow::Owned(document(d).into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(if path.trim_end_matches('/') == "icons" {
            ICONS
                .iter()
                .map(|(name, _)| format!("icons/{name}.svg").into())
                .chain(["icons/window-close.svg".into(), SPINNER.into()])
                .collect()
        } else {
            Vec::new()
        })
    }
}

/// `svg(path)` in ui/app.ts:68-73 with the `svg` rule of ui/tokens.css as attributes.
fn document(d: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="#000" stroke-width="{ICON_STROKE}" stroke-linecap="round" stroke-linejoin="round"><path d="{d}"/></svg>"##
    )
}

/// The close cross on the window controls' 12px grid, stroked as ui/windowControls.css:44-50.
fn window_control(d: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12" fill="none" stroke="#000" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="{d}"/></svg>"##
    )
}

/// The named icon at `size` px in `color`. GPUI paints an `svg` in its own text colour only,
/// never one it would inherit, so every icon names its colour.
pub(crate) fn icon(name: &'static str, size: f32, color: impl Into<Hsla>) -> Svg {
    debug_assert!(
        name == "window-close" || ICONS.iter().any(|(icon, _)| *icon == name),
        "{name}"
    );
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// icons.ts's table as (name, path) pairs.
    fn shipping() -> Vec<(String, String)> {
        let source = crate::tokens::tests::source("ui/icons.ts");
        let table = source
            .split_once("export const ICONS = {")
            .and_then(|(_, rest)| rest.split_once("} as const;"))
            .expect("icons.ts keeps its ICONS table")
            .0;
        table
            .lines()
            .filter_map(|line| {
                let (name, rest) = line.trim().split_once(": '")?;
                Some((name.to_owned(), rest.strip_suffix("',")?.to_owned()))
            })
            .collect()
    }

    #[test]
    fn the_icon_table_is_the_shipping_one() {
        let ours: Vec<_> = ICONS
            .iter()
            .map(|(name, d)| ((*name).to_owned(), (*d).to_owned()))
            .collect();
        assert_eq!(ours, shipping());
    }

    #[test]
    fn every_icon_is_served_as_an_svg() {
        for (name, d) in ICONS {
            let bytes = Assets.load(&format!("icons/{name}.svg")).unwrap().unwrap();
            let document = std::str::from_utf8(&bytes).unwrap();
            assert!(document.contains(&format!(r#"d="{d}""#)), "{name}");
            assert!(document.contains(r#"stroke-width="1.5""#));
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons").unwrap().len(), ICONS.len() + 2);
        assert!(Assets.load(SPINNER).unwrap().is_some());
        let close = Assets.load("icons/window-close.svg").unwrap().unwrap();
        assert!(std::str::from_utf8(&close).unwrap().contains(WINDOW_CLOSE));
        let shipping = crate::tokens::tests::source("ui/flechinha.html");
        assert!(shipping.contains(&format!(r#"<path d="{WINDOW_CLOSE}" />"#)));
        let rule = crate::tokens::tests::source("ui/windowControls.css");
        assert!(rule.contains("stroke-width: 1.4;"));
    }
}
