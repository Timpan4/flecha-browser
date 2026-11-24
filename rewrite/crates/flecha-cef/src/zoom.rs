use cef::ImplBrowserHost as _;
use flecha_core::TabId;

use crate::Engine;

// Chromium's Blink page_zoom.cc defines the desktop presets, multiplier and comparison tolerance.
const FACTORS: &[f64] = &[
    0.25,
    1.0 / 3.0,
    0.5,
    2.0 / 3.0,
    0.75,
    0.8,
    0.9,
    1.0,
    1.1,
    1.25,
    1.5,
    1.75,
    2.0,
    2.5,
    3.0,
    4.0,
    5.0,
];
const BASE: f64 = 1.2;
const EPSILON: f64 = 0.001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageZoom {
    In,
    Out,
    Reset,
}

impl PageZoom {
    pub fn level(self, current_level: f64) -> f64 {
        let current = BASE.powf(current_level);
        let factor = match self {
            Self::In => FACTORS
                .iter()
                .copied()
                .find(|step| *step > current + EPSILON)
                .unwrap_or(5.0),
            Self::Out => FACTORS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current - EPSILON)
                .unwrap_or(0.25),
            Self::Reset => return 0.0,
        };
        factor.ln() / BASE.ln()
    }
}

pub fn zoom_percentage(level: f64) -> u16 {
    (BASE.powf(level) * 100.0).round() as u16
}

impl Engine {
    pub fn zoom_level(&self, tab: &TabId) -> Option<f64> {
        self.host(tab).map(|host| host.zoom_level())
    }

    pub fn set_zoom_level(&self, tab: &TabId, level: f64) {
        if level.is_finite()
            && let Some(host) = self.host(tab)
        {
            host.set_zoom_level(level);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_through_chromiums_presets_and_clamps() {
        let mut level = 0.0;
        for expected in [110, 125, 150, 175, 200, 250, 300, 400, 500, 500] {
            level = PageZoom::In.level(level);
            assert_eq!(zoom_percentage(level), expected);
        }
        assert_eq!(PageZoom::Reset.level(level), 0.0);
        for expected in [
            400, 300, 250, 200, 175, 150, 125, 110, 100, 90, 80, 75, 67, 50, 33, 25, 25,
        ] {
            level = PageZoom::Out.level(level);
            assert_eq!(zoom_percentage(level), expected);
        }
        assert_eq!(zoom_percentage(PageZoom::Reset.level(level)), 100);
    }
}
