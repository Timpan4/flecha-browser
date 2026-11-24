use ashpd::desktop::print::{OutputFileFormat, PrintOptions, PrintProxy};
use gpui::App;

use crate::Controller;

impl Controller {
    pub(crate) fn poll_print_jobs(&mut self, cx: &mut App) {
        while let Some((tab, file)) = self.engine.next_print_job() {
            if self.quitting {
                continue;
            }
            let Ok(file) = file else {
                let session = self
                    .engine
                    .partition(&tab)
                    .and_then(flecha_cef::Partition::incognito_session);
                self.status
                    .set(session, "Could not render the page for printing.");
                cx.refresh_windows();
                continue;
            };
            if self.verify {
                crate::verify::print_ready(&tab, &file);
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
                if result.is_err_and(|error| !matches!(error, ashpd::Error::Response(_))) {
                    cx.update(|cx| {
                        let _ = handle.update(cx, |shell, _, cx| {
                            let mut state = shell.state.borrow_mut();
                            let session = state.model.incognito_session(&shell.id);
                            state
                                .status
                                .set(session, "Could not open the system print dialog.");
                            cx.notify();
                        });
                    });
                }
            })
            .detach();
        }
    }
}
