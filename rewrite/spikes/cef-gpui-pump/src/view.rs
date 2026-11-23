//! The GPUI window: the page's latest OnPaint frame as an image, a header with
//! counters, and an optional GPUI animation.

use crate::engine::Hub;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    Animation, AnimationExt as _, Context, ObjectFit, RenderImage, Window, div, img, prelude::*,
    px, rgb,
};
use image::{Frame, ImageBuffer};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub const HEADER_HEIGHT: f32 = 32.0;

#[derive(Default)]
pub struct ViewStats {
    /// Calls to `PageView::render`, which GPUI makes once per drawn frame.
    pub renders: Cell<u64>,
    /// Calls to the GPUI animation's animator closure.
    pub animation_steps: Cell<u64>,
    /// OnPaint frames turned into GPUI images.
    pub frames_uploaded: Cell<u64>,
    pub images_dropped: Cell<u64>,
    /// OnPaint frames replaced before GPUI drew them.
    pub frames_never_drawn: Cell<u64>,
}

pub struct PageView {
    hub: Rc<Hub>,
    stats: Rc<ViewStats>,
    current: Option<Arc<RenderImage>>,
    /// Whether `current` was drawn, so it may hold atlas tiles.
    current_drawn: bool,
    /// Drawn images replaced since the last frame; their tiles leave the atlas
    /// at the next render.
    retired: Vec<Arc<RenderImage>>,
    pub gpui_animation: bool,
}

impl PageView {
    pub fn new(
        hub: Rc<Hub>,
        stats: Rc<ViewStats>,
        mut frames: mpsc::UnboundedReceiver<()>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            while frames.next().await.is_some() {
                let updated = this.update(cx, |view, cx| {
                    if view.take_frame() {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            hub,
            stats,
            current: None,
            current_drawn: false,
            retired: Vec::new(),
            gpui_animation: false,
        }
    }

    fn take_frame(&mut self) -> bool {
        let Some(frame) = self.hub.frame.borrow_mut().take() else {
            return false;
        };
        // RenderImage expects BGRA, which is what OnPaint delivers.
        let Some(buffer) = ImageBuffer::from_raw(frame.width, frame.height, frame.bgra) else {
            return false;
        };
        let image = Arc::new(RenderImage::new([Frame::new(buffer)]));
        // A frame that was never drawn has no atlas tiles; dropping it frees it,
        // so frames arriving faster than GPUI draws do not pile up.
        if let Some(old) = self.current.replace(image) {
            if std::mem::replace(&mut self.current_drawn, false) {
                self.retired.push(old);
            } else {
                let skipped = &self.stats.frames_never_drawn;
                skipped.set(skipped.get() + 1);
            }
        }
        self.stats
            .frames_uploaded
            .set(self.stats.frames_uploaded.get() + 1);
        true
    }

    /// Drops all images so the window releases their atlas tiles.
    pub fn release_images(&mut self, window: &mut Window) {
        self.retired.extend(self.current.take());
        self.current_drawn = false;
        self.drop_retired(window);
    }

    fn drop_retired(&mut self, window: &mut Window) {
        // Each RenderImage has a new id, so replaced frames must leave the atlas.
        for image in self.retired.drain(..) {
            if window.drop_image(image).is_ok() {
                self.stats
                    .images_dropped
                    .set(self.stats.images_dropped.get() + 1);
            }
        }
    }
}

impl Render for PageView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.stats.renders.set(self.stats.renders.get() + 1);
        self.drop_retired(window);
        self.current_drawn = self.current.is_some();

        let width = px(self.hub.view_width as f32);
        let height = px(self.hub.view_height as f32);
        let mut header = div()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap_4()
            .px_2()
            .bg(rgb(0x202830))
            .text_color(rgb(0xffffff))
            .child(format!(
                "gpui renders {} | paints {} | uploads {}",
                self.stats.renders.get(),
                self.hub.paints(0),
                self.stats.frames_uploaded.get()
            ));
        if self.gpui_animation {
            let steps = self.stats.clone();
            header = header.child(
                div()
                    .relative()
                    .w(px(240.))
                    .h(px(16.))
                    .bg(rgb(0x334455))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .size(px(16.))
                            .bg(rgb(0xff8800))
                            .with_animation(
                                "gpui-slider",
                                Animation::new(Duration::from_secs(1)).repeat(),
                                move |el, delta| {
                                    steps.animation_steps.set(steps.animation_steps.get() + 1);
                                    el.left(px(224. * delta))
                                },
                            ),
                    ),
            );
        }
        let page = div().w(width).h(height).bg(rgb(0x000000));
        let page = match &self.current {
            Some(image) => page.child(
                img(image.clone())
                    .w(width)
                    .h(height)
                    .object_fit(ObjectFit::Fill),
            ),
            None => page,
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x101010))
            .child(header)
            .child(page)
    }
}
