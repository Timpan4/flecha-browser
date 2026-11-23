//! The idle check's frame rule: on a static page, CEF may paint only once in
//! the measured window, and only for Chromium's paint-holding timeout.
//!
//! Chromium 158 `third_party/blink/common/widget/constants.cc` sets
//! `kNewContentRenderingDelay = base::Seconds(4)`, the paint-holding timeout.
//! `RenderWidgetHostImpl::InitializePaintHolding` starts it when the navigation
//! commits, and on expiry `ClearDisplayedGraphics` makes the renderer repaint once.

use crate::schedule::MAX_PUMP_INTERVAL;
use std::time::Duration;

pub const PAINT_HOLDING_TIMEOUT: Duration = Duration::from_secs(4);

/// How long after `PAINT_HOLDING_TIMEOUT` past the navigation's first paint
/// the repaint may arrive. The timer starts at commit, before the first paint,
/// so the repaint is due by then; CEF delivers it inside a pump call, and calls
/// come at most `MAX_PUMP_INTERVAL` apart (`cef_app.h`). Measured: the repaint
/// came 27 ms before to 7 ms after first paint + 4 s in every idle run so far.
pub const REPAINT_DELIVERY: Duration = MAX_PUMP_INTERVAL;

/// One `OnPaint` of the page, in arrival order.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub after_navigation: Duration,
    /// The dirty rect list is exactly the whole view.
    pub full_view: bool,
    pub in_window: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Verdict {
    /// Per paint: whether it is the navigation's paint-holding repaint.
    pub paint_holding: Vec<bool>,
    pub paints_in_window: usize,
    pub paint_holding_in_window: usize,
    pub pass: bool,
}

/// The paint-holding repaint is a full-view paint that follows the
/// navigation's first paint, comes at least `PAINT_HOLDING_TIMEOUT` after
/// navigation, and no later than `PAINT_HOLDING_TIMEOUT + REPAINT_DELIVERY`
/// after that first paint. Only the first such paint counts, over the whole
/// navigation, not only the window: Chromium clears paint holding once per
/// navigation, so any later full-view paint is unexplained.
///
/// Passes when the pump made no unexplained call, every paint in the window is
/// that repaint, and GPUI drew no more frames than CEF painted in the window.
pub fn judge(paints: &[Paint], renders_in_window: u64, unexplained_calls: u64) -> Verdict {
    let latest = paints
        .first()
        .map(|first| first.after_navigation + PAINT_HOLDING_TIMEOUT + REPAINT_DELIVERY);
    let mut candidates = 0usize;
    let paint_holding: Vec<bool> = paints
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let candidate = i > 0
                && p.full_view
                && p.after_navigation >= PAINT_HOLDING_TIMEOUT
                && latest.is_some_and(|latest| p.after_navigation <= latest);
            candidates += candidate as usize;
            candidate && candidates <= 1
        })
        .collect();
    let paints_in_window = paints.iter().filter(|p| p.in_window).count();
    let paint_holding_in_window = paints
        .iter()
        .zip(&paint_holding)
        .filter(|(p, holding)| p.in_window && **holding)
        .count();
    let pass = unexplained_calls == 0
        && paints_in_window == paint_holding_in_window
        && renders_in_window <= paints_in_window as u64;
    Verdict {
        paint_holding,
        paints_in_window,
        paint_holding_in_window,
        pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(ms: u64, full_view: bool, in_window: bool) -> Paint {
        Paint {
            after_navigation: Duration::from_millis(ms),
            full_view,
            in_window,
        }
    }

    /// What the r4 and r5 idle runs saw: the first paint, then the 4 s repaint
    /// in the window.
    #[test]
    fn the_paint_holding_repaint_in_the_window_passes() {
        for log in [
            [paint(123, true, false), paint(4099, true, true)],
            [paint(97, true, false), paint(4099, true, true)],
        ] {
            let v = judge(&log, 1, 0);
            assert_eq!(v.paint_holding, [false, true]);
            assert!(v.pass, "{v:?}");
        }
        assert!(
            judge(&[paint(90, true, false)], 0, 0).pass,
            "no paint in the window"
        );
    }

    #[test]
    fn a_second_repaint_in_the_window_fails() {
        let log = [
            paint(90, true, false),
            paint(4099, true, true),
            paint(4110, true, true),
        ];
        let v = judge(&log, 2, 0);
        assert_eq!(v.paint_holding_in_window, 1);
        assert!(!v.pass, "{v:?}");
    }

    #[test]
    fn a_full_view_repaint_before_the_timeout_fails() {
        let log = [paint(90, true, false), paint(3500, true, true)];
        assert!(!judge(&log, 1, 0).pass);
    }

    /// Review thread scenario.rs:241: the real repaint fell before the window,
    /// so a later full-view paint in the window is not it, at 8 s or even
    /// inside the delivery bound.
    #[test]
    fn a_repaint_after_an_earlier_paint_holding_repaint_fails() {
        for late in [4110, 8000] {
            let log = [
                paint(90, true, false),
                paint(4050, true, false),
                paint(late, true, true),
            ];
            let v = judge(&log, 1, 0);
            assert_eq!(v.paint_holding, [false, true, false]);
            assert!(!v.pass, "{v:?}");
        }
    }

    /// Review thread 4665850156: with no earlier candidate, a lone repaint at
    /// 8 s is still too late to be the paint-holding repaint.
    #[test]
    fn a_lone_late_repaint_fails() {
        let log = [paint(90, true, false), paint(8000, true, true)];
        let v = judge(&log, 1, 0);
        assert_eq!(v.paint_holding, [false, false]);
        assert!(!v.pass, "{v:?}");
        let edge = 90 + 4000 + REPAINT_DELIVERY.as_millis() as u64;
        assert!(judge(&[paint(90, true, false), paint(edge, true, true)], 1, 0).pass);
        assert!(!judge(&[paint(90, true, false), paint(edge + 1, true, true)], 1, 0).pass);
    }

    /// With no earlier paint there is no first paint to time the repaint from.
    #[test]
    fn a_repaint_that_is_the_first_paint_fails() {
        assert!(!judge(&[paint(4099, true, true)], 1, 0).pass);
    }

    #[test]
    fn a_partial_repaint_or_extra_renders_or_unexplained_calls_fail() {
        let first = paint(90, true, false);
        assert!(!judge(&[first, paint(4099, false, true)], 1, 0).pass);
        assert!(!judge(&[first, paint(4099, true, true)], 2, 0).pass);
        assert!(!judge(&[first, paint(4099, true, true)], 1, 1).pass);
    }
}
