//! "About Flecha" from the window menu: what this build is, the Chromium and CEF it runs on,
//! and how its pages reach the screen. The shipping menu opens Chromium's own About page
//! (`api.openAbout` in chromium/shell/ui/app.ts), which the rewrite does not have.

use gpui::{
    ClickEvent, Context, Div, FontWeight, GpuSpecs, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, overlay_panel, primary_button};

/// What the dialog shows, read when it opens.
pub(crate) struct About {
    rows: Vec<(&'static str, String)>,
}

impl About {
    fn read(shares_textures: bool, gpu: Option<GpuSpecs>) -> Self {
        Self {
            rows: vec![
                ("Version", env!("CARGO_PKG_VERSION").to_owned()),
                ("Chromium", flecha_cef::chromium_version()),
                ("CEF", flecha_cef::cef_version().to_owned()),
                ("Compositor", compositor(shares_textures).to_owned()),
                ("GPU", gpu_line(gpu)),
            ],
        }
    }
}

fn compositor(shares_textures: bool) -> &'static str {
    if shares_textures {
        "Shared GPU textures"
    } else {
        "CPU frame copies"
    }
}

/// The renderer's device as Vulkan names it, and whether it is a software one.
fn gpu_line(gpu: Option<GpuSpecs>) -> String {
    let Some(gpu) = gpu else {
        return "Not reported".to_owned();
    };
    let driver = [gpu.driver_name, gpu.driver_info]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut line = gpu.device_name;
    if !driver.is_empty() {
        line = format!("{line} ({driver})");
    }
    if gpu.is_software_emulated {
        line.push_str(", software");
    }
    line
}

impl Shell {
    pub(crate) fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shares_textures = self.state.borrow().engine.shares_textures();
        let about = About::read(shares_textures, window.gpu_specs());
        self.show_overlay(Overlay::About(about), window, cx);
    }

    pub(crate) fn about_panel(&self, about: &About, cx: &Context<Self>) -> Div {
        overlay_panel()
            .gap_0()
            .p(px(tokens::DIALOG_PAD))
            .child(
                div()
                    .text_size(px(tokens::TEXT_LG))
                    .font_weight(FontWeight::BOLD)
                    .child("About Flecha"),
            )
            .child(
                // The names in one column and the values in the next, row for row.
                div()
                    .mt(px(tokens::DIALOG_TEXT_GAP_TOP))
                    .mb(px(tokens::DIALOG_TEXT_GAP_BOTTOM))
                    .flex()
                    .gap(px(tokens::SPACE_3))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .gap(px(tokens::SPACE_1))
                            .text_color(palette().text_2)
                            .children(about.rows.iter().map(|(name, _)| div().child(*name))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(tokens::SPACE_1))
                            .children(
                                about
                                    .rows
                                    .iter()
                                    .map(|(_, value)| div().truncate().child(value.clone())),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .child(primary_button("about-done", "Done").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                    ))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_about_rows_name_the_build_engine_and_renderer() {
        let gpu = GpuSpecs {
            is_software_emulated: true,
            device_name: "llvmpipe".into(),
            driver_name: "Mesa".into(),
            driver_info: String::new(),
        };
        let about = About::read(false, Some(gpu));
        let row = |name| {
            about
                .rows
                .iter()
                .find(|(row, _)| *row == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(row("Version"), Some(env!("CARGO_PKG_VERSION")));
        assert!(row("Chromium").unwrap().starts_with(char::is_numeric));
        assert!(!row("CEF").unwrap().contains('\0'));
        assert_eq!(row("Compositor"), Some("CPU frame copies"));
        assert_eq!(row("GPU"), Some("llvmpipe (Mesa), software"));
        assert_eq!(gpu_line(None), "Not reported");
        assert_eq!(compositor(true), "Shared GPU textures");
    }
}
