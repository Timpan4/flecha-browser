//! How CEF hands page frames to GPUI: shared GPU textures or CPU copies.
//!
//! In shared-texture mode CEF calls `OnAcceleratedPaint` with a dma-buf, and
//! `flecha_surface` copies it into a texture on GPUI's device. CEF's GPU process must
//! allocate that dma-buf on the GPU GPUI renders with, so the app probes GPUI's adapter
//! before CEF starts and passes its render node to Chromium. Without a usable match,
//! every browser gets CPU copies through `OnPaint`.
//!
//! CEF fixes a browser's mode when it creates the browser. When a shared texture cannot
//! be imported, the engine reopens that browser's page with CPU copies, and every
//! browser created after that gets CPU copies. CEF also falls back to `OnPaint` by
//! itself when GPU compositing becomes unavailable, and the render handler accepts those
//! frames in either mode.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use flecha_surface::SurfaceTarget;

/// Writes one diagnostic line to stderr. CEF callbacks cannot unwind, and `eprintln!`
/// panics when stderr rejects the write, so a lost line must not abort the browser.
pub(crate) fn log(line: std::fmt::Arguments<'_>) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{line}");
}

macro_rules! log {
    ($($arg:tt)*) => { log(format_args!($($arg)*)) };
}

/// How this process's browsers receive frames. The app decides once, before CEF starts.
#[derive(Clone, Default)]
pub struct Compositing {
    shared: Option<Shared>,
}

#[derive(Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Shared {
    target: SurfaceTarget,
    render_node: PathBuf,
    ozone: &'static str,
}

impl Compositing {
    /// CPU copies only, for runs without a window.
    pub fn software() -> Self {
        Self::default()
    }

    /// Shared textures when the GPU that renders `window` can import CEF's.
    pub fn for_window(window: &gpui::Window) -> Self {
        #[cfg(target_os = "linux")]
        {
            let ozone = flecha_surface::ozone_platform(window);
            let target = SurfaceTarget::for_window(window);
            match target.render_node(ozone) {
                Ok(render_node) => {
                    log!("page frames: shared textures on {}", render_node.display());
                    Self {
                        shared: Some(Shared {
                            target,
                            render_node,
                            ozone,
                        }),
                    }
                }
                Err(error) => {
                    log!("page frames: CPU copies, shared textures are unavailable: {error:#}");
                    Self::software()
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = window;
            Self::software()
        }
    }

    /// The render node Chromium must allocate shared textures on.
    pub(crate) fn render_node(&self) -> Option<&std::path::Path> {
        self.shared
            .as_ref()
            .map(|shared| shared.render_node.as_path())
    }
}

/// What a change of GPUI's device means for browsers that share textures.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DeviceChange {
    None,
    /// GPUI recreated its device on the same GPU. CEF keeps producing for it.
    SameGpu,
    /// GPUI now renders on another GPU or none, so shared textures are off.
    Lost,
}

/// The engine's current mode, shared with every browser's render handler.
#[derive(Default)]
pub(crate) struct Compositor {
    /// GPUI's renderer and CEF's render node, while browsers on that GPU may share.
    shared: RefCell<Option<Shared>>,
    /// Whether browsers created now share textures. One failed import clears it, and
    /// browsers that still import keep sharing.
    for_new_browsers: Cell<bool>,
}

impl Compositor {
    pub fn start(&self, compositing: &Compositing) {
        self.shared.replace(compositing.shared.clone());
        self.for_new_browsers.set(compositing.shared.is_some());
    }

    /// Whether browsers created now should share textures.
    pub fn shares_textures(&self) -> bool {
        self.for_new_browsers.get()
    }

    /// GPUI's renderer pair for imports, while it is on CEF's GPU.
    #[cfg(target_os = "linux")]
    pub fn target(&self) -> Option<SurfaceTarget> {
        self.shared
            .borrow()
            .as_ref()
            .map(|shared| shared.target.clone())
    }

    /// Gives browsers created from now on CPU copies. Logs only the change.
    pub fn turn_off(&self, reason: &str) {
        if self.for_new_browsers.replace(false) {
            log!("page frames: CPU copies for new browsers from now on: {reason}");
        }
    }

    /// Follows GPUI's device for `target`'s window.
    #[cfg(target_os = "linux")]
    pub fn follow(&self, target: SurfaceTarget) -> DeviceChange {
        let mut shared = self.shared.borrow_mut();
        let Some(current) = shared.as_mut() else {
            return DeviceChange::None;
        };
        if current.target.is_same_device(&target) {
            return DeviceChange::None;
        }
        match target.render_node(current.ozone) {
            Ok(node) if node == current.render_node => {
                current.target = target;
                DeviceChange::SameGpu
            }
            result => {
                let reason = match result {
                    Ok(node) => format!("GPUI moved to {}", node.display()),
                    Err(error) => format!("GPUI's new device cannot import: {error:#}"),
                };
                shared.take();
                drop(shared);
                self.turn_off(&reason);
                DeviceChange::Lost
            }
        }
    }
}

/// How a browser last delivered a frame, so a change is logged once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum Delivery {
    SharedTexture,
    CpuCopy,
}

/// Records `delivery` for a browser and logs when it differs from the last frame's.
pub(crate) fn delivered(last: &Cell<Option<Delivery>>, delivery: Delivery, browser: Option<i32>) {
    if last.replace(Some(delivery)) != Some(delivery) {
        let what = match delivery {
            Delivery::SharedTexture => "shared textures",
            Delivery::CpuCopy => "CPU copies",
        };
        match browser {
            Some(id) => log!("page frames: browser {id} receives {what}"),
            None => log!("page frames: a new browser receives {what}"),
        }
    }
}

/// Imports `info`. Debug builds started with `FLECHA_TEST_REJECT_SHARED_TEXTURE` pass the
/// process's first shared texture with no planes, so the importer rejects it and the
/// display harness can check the fallback.
#[cfg(target_os = "linux")]
pub(crate) fn reject_for_test<T>(
    info: &cef::AcceleratedPaintInfo,
    import: impl FnOnce(&cef::AcceleratedPaintInfo) -> T,
) -> T {
    #[cfg(debug_assertions)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static REJECTED: AtomicBool = AtomicBool::new(false);
        if std::env::var_os("FLECHA_TEST_REJECT_SHARED_TEXTURE").is_some()
            && !REJECTED.swap(true, Ordering::Relaxed)
        {
            let mut invalid = info.clone();
            invalid.plane_count = 0;
            return import(&invalid);
        }
    }
    import(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failed_import_gives_new_browsers_cpu_copies_and_keeps_the_target() {
        let compositor = Compositor::default();
        compositor.start(&Compositing {
            shared: Some(Shared {
                target: SurfaceTarget::default(),
                render_node: PathBuf::from("/dev/dri/renderD128"),
                ozone: "wayland",
            }),
        });
        assert!(compositor.shares_textures());
        compositor.turn_off("import failed");
        assert!(!compositor.shares_textures());
        // Browsers that already share keep importing on the same GPU.
        #[cfg(target_os = "linux")]
        assert!(compositor.target().is_some());
    }

    #[test]
    fn software_compositing_never_shares() {
        let compositor = Compositor::default();
        compositor.start(&Compositing::software());
        assert!(!compositor.shares_textures());
        assert_eq!(Compositing::software().render_node(), None);
    }
}
