mod input;
#[cfg(target_os = "linux")]
mod linux;

use std::{
    cell::{Cell, RefCell},
    io::{self, BufRead as _},
    ptr,
    rc::Rc,
    sync::{Arc, mpsc},
};

use anyhow::{Context as _, Result, bail, ensure};
use base64::Engine as _;
use cef::{rc::Rc as _, *};
use flecha_surface::{Frame as SurfaceFrame, SurfaceTarget, surface};
use gpui::{
    App as GpuiApp, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Pixels, QuitMode, Render, Styled as _, Window,
    WindowBounds, WindowOptions, div, point, px, rgb, size,
};

#[derive(Default)]
struct Options {
    software: bool,
    control: bool,
}

wrap_app! {
    struct CefApp {
        ready: Rc<Cell<bool>>,
        ozone: Option<&'static str>,
        render_node: Option<String>,
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ProcessHandler::new(self.ready.clone()))
        }

        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            if let Some(command_line) = command_line {
                command_line.append_switch(Some(&"no-startup-window".into()));
                if let Some(ozone) = self.ozone {
                    command_line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&ozone.into()));
                }
                if let Some(node) = &self.render_node {
                    command_line.append_switch_with_value(Some(&"render-node-override".into()), Some(&node.as_str().into()));
                    command_line.append_switch(Some(&"enable-gpu".into()));
                }
            }
        }
    }
}

wrap_browser_process_handler! {
    struct ProcessHandler { ready: Rc<Cell<bool>> }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            self.ready.set(true);
        }
    }
}

#[derive(Clone, PartialEq)]
struct Geometry {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    scale: f32,
}

impl Geometry {
    fn from_window(window: &Window) -> Self {
        let viewport = window.viewport_size();
        let bounds = window.bounds();
        Self {
            x: f32::from(bounds.origin.x).round() as i32,
            y: f32::from(bounds.origin.y).round() as i32,
            width: (f32::from(viewport.width).round() as i32).max(1),
            height: (f32::from(viewport.height).round() as i32).max(1),
            scale: window.scale_factor(),
        }
    }

    fn view(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }
}

struct Frames {
    target: SurfaceTarget,
    geometry: Geometry,
    view: Option<SurfaceFrame>,
    popup: Option<SurfaceFrame>,
    popup_bounds: Option<Bounds<Pixels>>,
    popup_visible: bool,
    dirty: bool,
    import_failure: Option<anyhow::Error>,
    reject_next_import: bool,
    view_change: Option<(&'static str, i32, i32)>,
}

impl Frames {
    fn push(&mut self, kind: PaintElementType, frame: SurfaceFrame) {
        match kind {
            PaintElementType::VIEW => self.view = Some(frame),
            PaintElementType::POPUP => {
                if self.popup.is_none() {
                    eprintln!("CEF popup paint");
                }
                self.popup = Some(frame);
            }
            _ => return,
        }
        self.dirty = true;
    }
}

wrap_render_handler! {
    struct OsrHandler {
        frames: Rc<RefCell<Frames>>,
        accelerated: bool,
        view_size: Rc<Cell<Option<(i32, i32)>>>,
    }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                *rect = self.frames.borrow().geometry.view();
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen: Option<&mut ScreenInfo>) -> i32 {
            let Some(screen) = screen else { return 0 };
            let state = self.frames.borrow();
            screen.device_scale_factor = state.geometry.scale;
            screen.rect = state.geometry.view();
            screen.available_rect = state.geometry.view();
            1
        }

        fn screen_point(
            &self, _browser: Option<&mut Browser>, view_x: i32, view_y: i32,
            screen_x: Option<&mut i32>, screen_y: Option<&mut i32>,
        ) -> i32 {
            let geometry = &self.frames.borrow().geometry;
            if let Some(x) = screen_x { *x = geometry.x.saturating_add(view_x); }
            if let Some(y) = screen_y { *y = geometry.y.saturating_add(view_y); }
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: i32) {
            let mut state = self.frames.borrow_mut();
            state.popup_visible = show != 0;
            if !state.popup_visible {
                eprintln!("CEF popup hidden");
                state.popup = None;
                state.popup_bounds = None;
            }
            state.dirty = true;
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            let mut state = self.frames.borrow_mut();
            state.popup_bounds = rect.map(|rect| Bounds::new(
                point(px(rect.x as f32), px(rect.y as f32)),
                size(px(rect.width as f32), px(rect.height as f32)),
            ));
            state.popup = None;
            state.dirty = true;
        }

        fn on_paint(
            &self, _browser: Option<&mut Browser>, kind: PaintElementType, _dirty: Option<&[Rect]>,
            buffer: *const u8, width: i32, height: i32,
        ) {
            let mut state = self.frames.borrow_mut();
            if self.accelerated && kind == PaintElementType::VIEW {
                state.import_failure.get_or_insert_with(|| anyhow::anyhow!("CEF returned a software frame in shared-texture mode"));
                return;
            }
            // The CEF buffer is valid during this callback and copied synchronously.
            match unsafe { SurfaceFrame::copy_bgra(buffer, width, height, &state.target) } {
                Ok(frame) => {
                    if kind == PaintElementType::VIEW
                        && self.view_size.replace(Some((width, height))) != Some((width, height))
                    {
                        state.view_change = Some(("software", width, height));
                    }
                    state.push(kind, frame);
                }
                Err(error) => eprintln!("CEF OnPaint: {error:#}"),
            }
        }

        #[cfg(target_os = "linux")]
        fn on_accelerated_paint(
            &self, _browser: Option<&mut Browser>, kind: PaintElementType, _dirty: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            let mut state = self.frames.borrow_mut();
            if !self.accelerated || state.import_failure.is_some() {
                return;
            }
            let force_error = std::mem::take(&mut state.reject_next_import);
            let (Some(gpu), Some(info)) = (&state.target.gpu, info) else {
                state.import_failure = Some(anyhow::anyhow!("missing CEF frame or GPUI device"));
                return;
            };
            let mut invalid;
            let info = if force_error {
                invalid = info.clone();
                invalid.plane_count = 0;
                &invalid
            } else {
                info
            };
            // Shared textures are enabled only after matching CEF's allocation node
            // to this GPUI Vulkan device. The importer waits while CEF owns the buffer.
            match unsafe { SurfaceFrame::copy_accelerated(info, gpu) } {
                Ok(frame) => {
                    let rect = &info.extra.visible_rect;
                    let size = (rect.width, rect.height);
                    if kind == PaintElementType::VIEW && self.view_size.replace(Some(size)) != Some(size) {
                        eprintln!("dma-buf frame: {}x{}, modifier {:#x}", rect.width, rect.height, info.modifier);
                        state.view_change = Some(("dma-buf", rect.width, rect.height));
                    }
                    state.push(kind, frame);
                }
                Err(error) => state.import_failure = Some(error),
            }
        }
    }
}

wrap_life_span_handler! {
    struct LifeSpan { closed: Rc<Cell<bool>> }
    impl LifeSpanHandler {
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.closed.set(true);
        }
    }
}

wrap_client! {
    struct OsrClient { render: RenderHandler, life: LifeSpanHandler }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.render.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.life.clone()) }
    }
}

struct Session {
    browser: Browser,
    closed: Rc<Cell<bool>>,
    accelerated: bool,
    closing: bool,
}

impl Session {
    fn create(frames: Rc<RefCell<Frames>>, accelerated: bool) -> Result<Self> {
        let url = CefString::from(
            format!(
                "data:text/html;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(include_str!("counter.html"))
            )
            .as_str(),
        );
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            windowless_rendering_enabled: 1,
            shared_texture_enabled: i32::from(accelerated),
            external_begin_frame_enabled: i32::from(accelerated),
            ..Default::default()
        };
        let closed = Rc::new(Cell::new(false));
        let mut client = OsrClient::new(
            OsrHandler::new(frames, accelerated, Rc::new(Cell::new(None))),
            LifeSpan::new(closed.clone()),
        );
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&url),
            Some(&BrowserSettings {
                background_color: 0xffff_ffff,
                ..Default::default()
            }),
            None,
            None,
        )
        .context("CEF could not create the Alloy OSR browser")?;
        let host = browser.host().context("CEF browser has no host")?;
        ensure!(
            host.runtime_style() == RuntimeStyle::ALLOY,
            "CEF did not create an Alloy browser"
        );
        eprintln!("created Alloy OSR browser (shared textures: {accelerated})");
        Ok(Self {
            browser,
            closed,
            accelerated,
            closing: false,
        })
    }

    fn close(&mut self) {
        if !self.closing {
            self.closing = true;
            if let Some(host) = self.browser.host() {
                host.close_browser(1);
            }
        }
    }
}

struct Demo {
    frames: Rc<RefCell<Frames>>,
    session: Option<Session>,
    ready: Rc<Cell<bool>>,
    accelerated: bool,
    closing: Rc<Cell<bool>>,
    focus: FocusHandle,
    focused: Option<bool>,
    retired_images: [Option<Arc<gpui::RenderImage>>; 2],
    controls: Option<mpsc::Receiver<Control>>,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
}

impl Demo {
    fn host(&self) -> Option<BrowserHost> {
        let session = self.session.as_ref()?;
        if session.closing || session.closed.get() {
            return None;
        }
        session.browser.host()
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(controls) = &self.controls {
            for command in controls.try_iter() {
                match command {
                    Control::Resize(width, height) => window.resize(size(px(width), px(height))),
                    Control::RejectNextImport => self.frames.borrow_mut().reject_next_import = true,
                    Control::Close => self.closing.set(true),
                }
            }
        }

        {
            let mut frames = self.frames.borrow_mut();
            let current_target = SurfaceTarget::for_window(window);
            #[cfg(target_os = "linux")]
            if self.accelerated
                && frames.target.gpu.as_ref().map(|gpu| &gpu.device)
                    != current_target.gpu.as_ref().map(|gpu| &gpu.device)
            {
                frames.import_failure = Some(anyhow::anyhow!("the GPUI device changed"));
            }
            frames.target = current_target;
        }

        let import_failure = self.frames.borrow_mut().import_failure.take();
        if let Some(error) = import_failure {
            eprintln!("accelerated OSR failed, creating a software browser: {error:#}");
            self.accelerated = false;
            if let Some(session) = &mut self.session {
                session.close();
            }
        }

        if self
            .session
            .as_ref()
            .is_some_and(|session| session.closed.get())
        {
            self.session = None;
            let mut frames = self.frames.borrow_mut();
            frames.popup = None;
            frames.popup_bounds = None;
            frames.popup_visible = false;
            frames.dirty = true;
        }

        if self.closing.get() {
            if let Some(session) = &mut self.session {
                session.close();
            } else {
                window.remove_window();
                return false;
            }
        } else if self.ready.get() && self.session.is_none() {
            self.frames.borrow_mut().geometry = Geometry::from_window(window);
            match Session::create(self.frames.clone(), self.accelerated) {
                Ok(session) => {
                    self.session = Some(session);
                    self.focused = None;
                }
                Err(error) => {
                    *self.failure.borrow_mut() = Some(error);
                    window.remove_window();
                    return false;
                }
            }
        }

        let geometry = Geometry::from_window(window);
        let (resized, screen_changed) = {
            let mut frames = self.frames.borrow_mut();
            let resized = frames.geometry.width != geometry.width
                || frames.geometry.height != geometry.height;
            let screen_changed = frames.geometry.scale != geometry.scale
                || frames.geometry.x != geometry.x
                || frames.geometry.y != geometry.y;
            if resized || screen_changed {
                eprintln!(
                    "OSR viewport: {}x{} @ {}",
                    geometry.width, geometry.height, geometry.scale
                );
                frames.geometry = geometry;
            }
            (resized, screen_changed)
        };
        if let Some(host) = self.host() {
            if screen_changed {
                host.notify_screen_info_changed();
            }
            if resized || screen_changed {
                host.was_resized();
            }
            let focused = window.is_window_active();
            if self.focused != Some(focused) {
                self.focused = Some(focused);
                host.set_focus(i32::from(focused));
            }
            if self
                .session
                .as_ref()
                .is_some_and(|session| session.accelerated)
            {
                host.send_external_begin_frame();
            }
        }

        do_message_loop_work();
        if std::mem::take(&mut self.frames.borrow_mut().dirty) {
            cx.notify();
        }
        true
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (view, popup) = {
            let mut frames = self.frames.borrow_mut();
            if let Some((mode, width, height)) = frames.view_change.take() {
                window.on_next_frame(move |_, _| {
                    eprintln!("composited {mode} frame: {width}x{height}");
                });
            }
            let popup = if frames.popup_visible {
                frames.popup_bounds.zip(frames.popup.clone())
            } else {
                None
            };
            (frames.view.clone(), popup)
        };
        let next_images = [view.as_ref(), popup.as_ref().map(|(_, frame)| frame)]
            .map(|frame| frame.and_then(SurfaceFrame::image).cloned());
        for (old, new) in self.retired_images.iter().zip(&next_images) {
            if let Some(old) = old
                && new.as_ref().is_none_or(|new| new.id != old.id)
                && let Err(error) = window.drop_image(old.clone())
            {
                eprintln!("release CEF atlas image: {error:#}");
            }
        }
        self.retired_images = next_images;

        let mut root = div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|demo, event: &gpui::KeyDownEvent, _, _| {
                if let Some(host) = demo.host() {
                    input::key(&host, &event.keystroke, false, event.is_held);
                }
            }))
            .on_key_up(cx.listener(|demo, event: &gpui::KeyUpEvent, _, _| {
                if let Some(host) = demo.host() {
                    input::key(&host, &event.keystroke, true, false);
                }
            }))
            .on_mouse_move(cx.listener(|demo, event: &gpui::MouseMoveEvent, _, _| {
                if let Some(host) = demo.host() {
                    input::mouse_move(&host, event);
                }
            }))
            .on_mouse_exit(cx.listener(|demo, event: &gpui::MouseExitEvent, _, _| {
                if let Some(host) = demo.host() {
                    input::mouse_exit(&host, event);
                }
            }))
            .on_scroll(cx.listener(|demo, event: &gpui::ScrollEvent, _, _| {
                if let Some(host) = demo.host() {
                    input::scroll(&host, event);
                }
            }))
            .child(surface(view, popup))
            .child(
                div()
                    .absolute()
                    .top(px(28.))
                    .right(px(24.))
                    .w(px(96.))
                    .h(px(40.))
                    .bg(rgb(0xf53ab5)),
            );
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            root = root
                .on_mouse_down(
                    button,
                    cx.listener(|demo, event: &gpui::MouseDownEvent, window, cx| {
                        demo.focus.focus(window, cx);
                        if let Some(host) = demo.host() {
                            input::click(
                                &host,
                                event.position,
                                event.modifiers,
                                event.button,
                                event.click_count,
                                false,
                            );
                        }
                    }),
                )
                .on_mouse_up(
                    button,
                    cx.listener(|demo, event: &gpui::MouseUpEvent, _, _| {
                        if let Some(host) = demo.host() {
                            input::click(
                                &host,
                                event.position,
                                event.modifiers,
                                event.button,
                                event.click_count,
                                true,
                            );
                        }
                    }),
                );
        }
        root
    }
}

fn pump(view: gpui::WeakEntity<Demo>, window: &Window) {
    window.on_next_frame(move |window, cx| {
        if let Some(entity) = view.upgrade()
            && entity.update(cx, |demo, cx| demo.tick(window, cx))
        {
            pump(view, window);
        }
    });
}

enum Control {
    Resize(f32, f32),
    RejectNextImport,
    Close,
}

fn controls() -> mpsc::Receiver<Control> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            let words: Vec<_> = line.split_whitespace().collect();
            let control = match words.as_slice() {
                ["resize", width, height] => {
                    let (Ok(width), Ok(height)) = (width.parse::<u16>(), height.parse::<u16>())
                    else {
                        continue;
                    };
                    if width == 0 || height == 0 {
                        continue;
                    }
                    Control::Resize(f32::from(width), f32::from(height))
                }
                ["close"] => {
                    let _ = tx.send(Control::Close);
                    return;
                }
                ["reject-next-import"] => Control::RejectNextImport,
                _ => continue,
            };
            if tx.send(control).is_err() {
                return;
            }
        }
        let _ = tx.send(Control::Close);
    });
    rx
}

fn start(
    window: &mut Window,
    cx: &mut GpuiApp,
    options: Options,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
    initialized: Rc<Cell<bool>>,
) -> Entity<Demo> {
    let target = SurfaceTarget::for_window(window);
    let (ozone, render_node) = platform_configuration(window, &target, options.software);
    let accelerated = render_node.is_some();
    let ready = Rc::new(Cell::new(false));
    let mut app = CefApp::new(ready.clone(), ozone, render_node);
    let mut settings = Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        command_line_args_disabled: 1,
        ..Default::default()
    };
    if let Some(runtime) = std::env::var_os("FLECHA_CEF_RUNTIME_DIR") {
        let runtime = std::path::PathBuf::from(runtime);
        settings.resources_dir_path = runtime.to_string_lossy().as_ref().into();
        settings.locales_dir_path = runtime.join("locales").to_string_lossy().as_ref().into();
    }
    if let Some(profile) = std::env::var_os("FLECHA_CEF_PROFILE_DIR") {
        settings.root_cache_path = profile.to_string_lossy().as_ref().into();
    }
    let args = args::Args::new();
    let did_initialize = initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        ptr::null_mut(),
    ) == 1;
    initialized.set(did_initialize);
    if !did_initialize {
        *failure.borrow_mut() = Some(anyhow::anyhow!("CEF initialization failed"));
    }
    let geometry = Geometry::from_window(window);
    eprintln!(
        "OSR viewport: {}x{} @ {}",
        geometry.width, geometry.height, geometry.scale
    );

    let closing = Rc::new(Cell::new(!did_initialize));
    window.on_window_should_close(cx, {
        let closing = closing.clone();
        move |_, _| {
            closing.set(true);
            false
        }
    });
    let demo = cx.new(|cx| Demo {
        frames: Rc::new(RefCell::new(Frames {
            target,
            geometry,
            view: None,
            popup: None,
            popup_bounds: None,
            popup_visible: false,
            dirty: false,
            import_failure: None,
            reject_next_import: false,
            view_change: None,
        })),
        session: None,
        ready,
        accelerated,
        closing,
        focused: None,
        focus: cx.focus_handle(),
        retired_images: [None, None],
        controls: options.control.then(controls),
        failure,
    });
    let focus = demo.read(cx).focus.clone();
    focus.focus(window, cx);
    pump(demo.downgrade(), window);
    demo
}

fn platform_configuration(
    _window: &Window,
    _target: &SurfaceTarget,
    software: bool,
) -> (Option<&'static str>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        let ozone = linux::ozone_platform(_window);
        if let Some(gpu) = &_target.gpu {
            eprintln!("GPUI renderer: {:?}", gpu.adapter.get_info());
        }
        let node = if software {
            None
        } else {
            match _target
                .gpu
                .as_ref()
                .context("GPUI did not provide a GPU")
                .and_then(|gpu| linux::render_node(gpu, ozone))
            {
                Ok(node) => {
                    eprintln!("CEF and GPUI render node: {}", node.display());
                    Some(node.to_string_lossy().into_owned())
                }
                Err(error) => {
                    eprintln!("shared textures unavailable, starting software OSR: {error:#}");
                    None
                }
            }
        };
        (Some(ozone), node)
    }
    #[cfg(not(target_os = "linux"))]
    {
        if !software {
            eprintln!(
                "native texture composition is unimplemented on this platform; starting software OSR"
            );
        }
        (None, None)
    }
}

fn run() -> Result<()> {
    #[cfg(target_os = "macos")]
    let _loader = {
        let exe = std::env::current_exe()?;
        let is_helper = std::env::args().any(|arg| arg.starts_with("--type="));
        let loader = library_loader::LibraryLoader::new(&exe, is_helper);
        ensure!(
            loader.load(),
            "could not load the CEF framework from the app bundle"
        );
        loader
    };
    ensure!(
        !api_hash(sys::CEF_API_VERSION_LAST, 0).is_null()
            && api_version() == sys::CEF_API_VERSION_LAST,
        "libcef does not expose the compiled CEF API version"
    );
    let args = args::Args::new();
    let child_code = execute_process(Some(args.as_main_args()), None, ptr::null_mut());
    if child_code >= 0 {
        std::process::exit(child_code);
    }

    let mut options = Options::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--software" => options.software = true,
            "--control" => options.control = true,
            "--help" => {
                println!("Usage: gpui-cef-compositor [--software] [--control]");
                return Ok(());
            }
            _ => bail!("unknown option: {arg}"),
        }
    }

    let failure = Rc::new(RefCell::new(None));
    let initialized = Rc::new(Cell::new(false));
    let shared_failure = failure.clone();
    let shared_initialized = initialized.clone();
    gpui_platform::application()
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            let options_window = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(800.), px(480.)),
                    cx,
                ))),
                titlebar: None,
                ..Default::default()
            };
            let start_failure = shared_failure.clone();
            if let Err(error) = cx.open_window(options_window, move |window, cx| {
                start(window, cx, options, start_failure, shared_initialized)
            }) {
                *shared_failure.borrow_mut() = Some(error);
                cx.quit();
            }
        });
    if initialized.get() {
        shutdown();
    }
    failure.take().map_or(Ok(()), Err)
}

fn main() -> Result<()> {
    run()
}
