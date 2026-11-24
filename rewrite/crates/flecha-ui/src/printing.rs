use ashpd::desktop::ResponseError;
use ashpd::desktop::print::{OutputFileFormat, PrintOptions, PrintProxy};
use gpui::{App, PromptLevel, WindowHandle};

use crate::Controller;
use crate::view::Shell;

fn show_print_error(handle: WindowHandle<Shell>, detail: &'static str, cx: &mut App) {
    let _ = handle.update(cx, |_, window, cx| {
        drop(window.prompt(
            PromptLevel::Warning,
            "Printing unavailable",
            Some(detail),
            &["OK"],
            cx,
        ));
    });
}

impl Controller {
    pub(crate) fn poll_print_jobs(&mut self, cx: &mut App) {
        while let Some((tab, file)) = self.engine.next_print_job() {
            if self.quitting {
                continue;
            }
            let Some(handle) = self
                .engine
                .key(&tab)
                .and_then(|key| self.browser_window(&key))
                .and_then(|window| self.windows.get(window))
                .copied()
            else {
                continue;
            };
            let Ok(file) = file else {
                cx.defer(move |cx| {
                    show_print_error(handle, "Could not render the page for printing.", cx)
                });
                continue;
            };
            if self.verify {
                crate::verify::print_ready(&tab, &file);
                continue;
            }
            cx.spawn(async move |cx| {
                let result = async {
                    // With no PreparePrint token, the portal asks the user before submitting.
                    let options = PrintOptions::default()
                        .set_supported_output_file_formats([OutputFileFormat::Pdf]);
                    PrintProxy::new()
                        .await?
                        .print(None, "Print page", &file, options)
                        .await?
                        .response()
                }
                .await;
                if result.is_err_and(|error| {
                    !matches!(error, ashpd::Error::Response(ResponseError::Cancelled))
                }) {
                    cx.update(|cx| {
                        show_print_error(handle, "Could not open the system print dialog.", cx)
                    });
                }
            })
            .detach();
        }
    }
}
