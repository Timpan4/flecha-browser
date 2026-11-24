use std::fs::File;
use std::io;
use std::rc::{Rc, Weak};

use anyhow::{Context as _, Result};
use cef::{rc::Rc as _, *};
use flecha_core::TabId;

use crate::callbacks::Slot;

const PDF_NAME: &str = "page.pdf";

wrap_print_handler! {
    pub(crate) struct LinuxPrinting;
    impl PrintHandler {
        fn pdf_paper_size(&self, _browser: Option<&mut Browser>, units_per_inch: i32) -> Size {
            // US Letter, the default size used by CEF's PDF settings.
            Size {
                width: units_per_inch.saturating_mul(17) / 2,
                height: units_per_inch.saturating_mul(11),
            }
        }
    }
}

wrap_pdf_print_callback! {
    struct PdfReady { slot: Weak<Slot>, directory: Rc<tempfile::TempDir> }
    impl PdfPrintCallback {
        fn on_pdf_print_finished(&self, _path: Option<&CefString>, ok: i32) {
            if let Some(slot) = self.slot.upgrade().filter(|slot| !slot.retired.get()) {
                let file = if ok != 0 {
                    File::open(self.directory.path().join(PDF_NAME))
                } else {
                    Err(io::Error::other("CEF could not render the print document"))
                };
                slot.popups.services.print_jobs.push(self.slot.clone(), file);
            }
        }
    }
}

impl crate::Engine {
    pub(crate) fn print_for_portal(&self, tab: &TabId, host: &BrowserHost) -> Result<()> {
        let slot = &self.browsers.get(tab).context("The page has closed")?.slot;
        let directory = Rc::new(tempfile::tempdir().context("Create a temporary print directory")?);
        let path = directory.path().join(PDF_NAME);
        let path = path.to_str().context("The print path is not UTF-8")?;
        let mut callback = PdfReady::new(Rc::downgrade(slot), directory);
        // The pinned Linux OSR runtime traps in its native print worker after OnPrintDialog.
        // Render the PDF through CEF and let the desktop portal submit it.
        host.print_to_pdf(
            Some(&CefString::from(path)),
            Some(&PdfPrintSettings::default()),
            Some(&mut callback),
        );
        Ok(())
    }

    pub fn next_print_job(&mut self) -> Option<(TabId, io::Result<File>)> {
        self.adopt_popups();
        while let Some((tab, file)) = self.services().print_jobs.pop() {
            if let Some(tab) = tab {
                return Some((tab, file));
            }
        }
        None
    }
}
