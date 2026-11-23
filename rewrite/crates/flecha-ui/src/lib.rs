mod model;
mod view;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{Engine, Event, Owner, Runtime, Viewport};
use flecha_core::{
    SpaceId, TabId, WindowId,
    storage::{LoadedState, ReadyState},
};
use flecha_services::StateStore;
use gpui::{App, Bounds, WindowBounds, WindowHandle, WindowOptions, px, size};

use model::{Model, now};
use view::Shell;

pub struct Options {
    pub initial: Option<String>,
    pub verify: bool,
}

pub fn start(
    cx: &mut App,
    runtime: &mut Runtime,
    profile: PathBuf,
    store: StateStore,
    mut loaded: LoadedState,
    options: Options,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
) -> Result<()> {
    let ready = loaded.apply_recovery(|write| store.write(write))?;
    let model = Model::new(ready.state.take());
    let initial = options.initial;
    let controller = Rc::new(RefCell::new(Controller {
        model,
        engine: Engine::new(profile),
        store,
        snapshots: loaded,
        windows: BTreeMap::new(),
        ready: false,
        quitting: false,
        verify: options.verify,
        failure,
        status: String::new(),
    }));
    sync_windows(&controller, cx)?;
    if options.verify {
        install_controls(&controller, cx);
    }
    if let Some(url) = initial {
        let window = controller.borrow().model.core.focused_window().id.clone();
        controller.borrow_mut().act(&window, Action::Open(url))?;
    }
    runtime.drive(cx, move |ready, cx| {
        let keep_running = controller.borrow_mut().tick(ready, cx);
        if keep_running {
            if let Err(error) = sync_windows(&controller, cx) {
                controller.borrow_mut().fail(error, cx);
            }
        }
        keep_running
    });
    Ok(())
}

#[derive(Clone)]
enum Action {
    Open(String),
    Select(TabId),
    Split(TabId),
    SplitNew(String),
    ExitSplit,
    Peek(String),
    Promote,
    ClosePeek,
    Detach,
    Return,
    Close,
    Switch(SpaceId),
    NewSpace,
    Isolate,
    Quit,
}

struct Controller {
    model: Model,
    engine: Engine,
    store: StateStore,
    snapshots: LoadedState,
    windows: BTreeMap<WindowId, WindowHandle<Shell>>,
    ready: bool,
    quitting: bool,
    verify: bool,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
    status: String,
}

impl Controller {
    fn snapshots(&mut self) -> &mut ReadyState {
        self.snapshots
            .ready()
            .expect("session recovery ran before opening windows")
    }

    fn save(&mut self) -> Result<()> {
        let write = self
            .snapshots
            .ready()
            .expect("session recovered")
            .save_current(&self.model.core)?;
        self.store.write(&write)
    }

    fn act(&mut self, window: &WindowId, action: Action) -> Result<()> {
        if self.quitting {
            return Ok(());
        }
        match action {
            Action::Quit => {
                self.quitting = true;
                self.engine.close_all();
                return self.save();
            }
            Action::Open(url) => {
                self.model.open(window, url, true)?;
            }
            Action::Select(id) => {
                self.model
                    .core
                    .activate(&id, window, Some(now()))
                    .removed
                    .iter()
                    .for_each(|id| self.engine.close(id));
            }
            Action::Split(right) => {
                if let Some(left) = self.model.active(window).cloned() {
                    self.split(window, left, right)?;
                }
            }
            Action::SplitNew(url) => {
                let left = self.model.active(window).cloned();
                let right = self.model.open(window, url, left.is_none())?;
                if let Some(left) = left {
                    self.split(window, left, right)?;
                }
            }
            Action::ExitSplit => {
                if let Some(flecha_core::EndedSplit::Responsive { mirror }) =
                    self.model.core.unsplit(window)
                {
                    self.engine.close(&mirror);
                }
            }
            Action::Peek(url) => {
                let source = self.model.active(window).cloned();
                if let Some(old) = self.model.peek(window.clone(), url, source.as_ref())? {
                    self.engine.close(&old);
                }
                return Ok(());
            }
            Action::Promote => self.model.promote(window)?,
            Action::ClosePeek => {
                if let Some(page) = self.model.peeks.remove(window) {
                    self.engine.close(&page.id);
                }
                return Ok(());
            }
            Action::Detach => {
                if let Some(tab) = self.model.active(window).cloned() {
                    for removed in self.model.to_flechinha(&tab)? {
                        self.engine.close(&removed);
                    }
                }
            }
            Action::Return => {
                let tab = self
                    .model
                    .flechinhas
                    .get(window)
                    .context("no Flechinha page")?
                    .id
                    .clone();
                self.model.from_flechinha(window)?;
                self.reconcile_partition(&tab);
            }
            Action::Close => {
                if let Some(page) = self.model.flechinhas.remove(window) {
                    self.engine.close(&page.id);
                    return Ok(());
                }
                if self.model.peeks.contains_key(window) {
                    return self.act(window, Action::ClosePeek);
                }
                if let Some(tab) = self.model.active(window).cloned() {
                    self.engine.close(&tab);
                    for removed in self.model.core.close_tab(&tab, now()).removed {
                        self.engine.close(&removed);
                    }
                }
            }
            Action::Switch(space) => {
                self.model
                    .core
                    .switch_space(window, &space)
                    .removed
                    .iter()
                    .for_each(|id| self.engine.close(id));
            }
            Action::NewSpace => {
                let id = model::space_id();
                ensure!(
                    self.model.core.add_space(id.clone()),
                    "could not create Space"
                );
                return self.act(window, Action::Switch(id));
            }
            Action::Isolate => {
                let id = self
                    .model
                    .core
                    .window(window)
                    .context("missing Flecha window")?
                    .active_space_id
                    .clone();
                let isolated = self
                    .model
                    .core
                    .space(&id)
                    .context("missing active Space")?
                    .isolated;
                for tab in self.model.core.set_isolated(&id, !isolated) {
                    self.reconcile_partition(&tab);
                }
            }
        }
        self.save()
    }

    fn split(&mut self, window: &WindowId, left: TabId, right: TabId) -> Result<()> {
        let split = self.model.core.set_split(window, [left, right]);
        ensure!(split.result, "could not split these tabs");
        for removed in split.removed {
            self.engine.close(&removed);
        }
        Ok(())
    }

    fn reconcile_partition(&mut self, tab: &TabId) {
        if self
            .engine
            .partition(tab)
            .is_some_and(|partition| Some(partition) != self.model.partition(tab).as_ref())
        {
            self.engine.close(tab);
        }
    }

    fn pane(&mut self, owner: Owner, id: TabId, viewport: Viewport) -> Result<()> {
        if !self.ready || self.quitting {
            return Ok(());
        }
        let Some(tab) = self.model.page(&owner, &id) else {
            return Ok(());
        };
        let partition = self
            .model
            .partition(&id)
            .context("page lacks a request context")?;
        self.engine.ensure(id, owner, partition, &tab.url, viewport)
    }

    fn fail(&mut self, error: anyhow::Error, cx: &mut App) {
        *self.failure.borrow_mut() = Some(error);
        self.quitting = true;
        self.engine.close_all();
        cx.refresh_windows();
    }

    fn tick(&mut self, ready: bool, cx: &mut App) -> bool {
        if ready && !self.ready {
            self.ready = true;
            if let Some(write) = self.snapshots().started()
                && let Err(error) = self.store.write(&write)
            {
                self.fail(error, cx);
            }
            cx.refresh_windows();
        }
        let mut changed = false;
        let mut save = false;
        for message in self.engine.take_events() {
            let id = &message.key.tab;
            match message.event {
                Event::Address(address) => {
                    if let Some((tab, durable)) = self.model.tab_mut(id)
                        && tab.url != address
                    {
                        tab.url = address;
                        save |= durable;
                        changed = true;
                    }
                }
                Event::Title(title) => {
                    if let Some((tab, durable)) = self.model.tab_mut(id)
                        && tab.title != title
                    {
                        tab.title = title;
                        save |= durable;
                        changed = true;
                    }
                }
                Event::Paint => {
                    changed = true;
                    if self.verify
                        && self
                            .engine
                            .frames(id)
                            .is_some_and(|frames| frames.view.is_some())
                    {
                        println!("cef paint");
                    }
                }
                Event::Loaded { status, .. } if self.verify => println!("cef load {status}"),
                Event::Created(browser) if self.verify => println!("cef created {browser}"),
                Event::Closed if self.verify => println!("cef closed"),
                Event::PaintFailed => {
                    self.fail(anyhow::anyhow!("CEF could not copy the page frame"), cx)
                }
                Event::OpenLink { .. }
                | Event::Loaded { .. }
                | Event::Created(_)
                | Event::Closed => {}
            }
        }
        if save && let Err(error) = self.save() {
            self.fail(error, cx);
        }
        if changed {
            cx.refresh_windows();
        }
        if self.quitting && self.engine.is_drained() {
            cx.quit();
            return false;
        }
        true
    }
}

type Shared = Rc<RefCell<Controller>>;

fn dispatch(controller: &Shared, window: &WindowId, action: Action, cx: &mut App) {
    let mut state = controller.borrow_mut();
    state.status.clear();
    if let Err(error) = state.act(window, action) {
        state.status = error.to_string();
        if state.quitting {
            state.fail(error, cx);
        }
    }
    cx.refresh_windows();
}

fn install_controls(controller: &Shared, cx: &mut App) {
    use futures::StreamExt as _;
    use std::io::BufRead as _;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.unbounded_send(line).is_err() {
                break;
            }
        }
    });
    let controller = controller.clone();
    cx.spawn(async move |cx| {
        while let Some(command) = rx.next().await {
            cx.update(|cx| {
                let id = controller.borrow().model.core.focused_window().id.clone();
                if command == "quit" {
                    dispatch(&controller, &id, Action::Quit, cx);
                }
            });
        }
    })
    .detach();
}

fn sync_windows(controller: &Shared, cx: &mut App) -> Result<()> {
    let (wanted, opened) = {
        let state = controller.borrow();
        let mut wanted = BTreeMap::new();
        for window in state.model.core.windows() {
            wanted.insert(window.id.clone(), false);
        }
        for id in state.model.flechinhas.keys() {
            wanted.insert(id.clone(), true);
        }
        (wanted, state.windows.clone())
    };
    for (id, handle) in &opened {
        if !wanted.contains_key(id) {
            controller.borrow_mut().windows.remove(id);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
    for (id, small) in wanted {
        if opened.contains_key(&id) {
            continue;
        }
        // Flechinha uses the shipping shell's 900 x 700 size. The main
        // window uses GPUI's default size.
        let bounds = small
            .then(|| WindowBounds::Windowed(Bounds::centered(None, size(px(900.), px(700.)), cx)));
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: bounds,
                ..Default::default()
            },
            {
                let controller = controller.clone();
                let id = id.clone();
                move |window, cx| {
                    window.set_window_title(if small { "Flechinha" } else { "Flecha" });
                    window.on_window_should_close(cx, {
                        let controller = controller.clone();
                        let id = id.clone();
                        move |_, cx| {
                            dispatch(
                                &controller,
                                &id,
                                if small { Action::Close } else { Action::Quit },
                                cx,
                            );
                            false
                        }
                    });
                    Shell::new(controller, id, window, cx)
                }
            },
        )?;
        controller.borrow_mut().windows.insert(id, handle);
    }
    Ok(())
}
