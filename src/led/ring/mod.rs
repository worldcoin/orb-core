//! Animations for the ring LEDs.

mod alert;
mod arc_dash;
mod arc_pulse;
mod fake_progress;
mod idle;
mod progress;
mod segmented;
mod slider;
mod spinner;

pub use self::{
    alert::Alert,
    arc_dash::{ArcDash, MAX_ARC_COUNT},
    arc_pulse::ArcPulse,
    fake_progress::FakeProgress,
    idle::Idle,
    progress::Progress,
    segmented::{Segment, Segmented},
    slider::Slider,
    spinner::Spinner,
};

use super::{Animation, RingFrame};
use crate::{
    led::GAMMA,
    mcu::main::{Rgb, RING_LED_COUNT},
};
use std::{f64::consts::PI, ops::Range};

const LIGHT_BLEEDING_OFFSET_RAD: f64 = PI / 180.0 * 6.0; // 6° offset of the start to compensate for light bleeding.

/// Renders a set of lines with smooth ends.
#[allow(clippy::cast_precision_loss)]
pub fn render_lines<const N: usize>(
    frame: &mut RingFrame,
    background: Rgb,
    foreground: Rgb,
    ranges_angle_rad: &[Range<f64>; N],
) {
    'leds: for (i, led) in frame.iter_mut().enumerate() {
        const LED: f64 = PI * 2.0 / RING_LED_COUNT as f64;
        let pos = i as f64 * LED;
        for &Range { start, end } in ranges_angle_rad {
            let start_fill = pos - start + LED;
            if start_fill <= 0.0 {
                continue;
            }
            let end_fill = end - pos;
            if end_fill <= 0.0 {
                continue;
            }
            *led = foreground;
            if start_fill < LED || end_fill < LED {
                *led *= ((start_fill.min(LED) + end_fill.min(LED) - LED) / LED).powf(GAMMA);
            }
            continue 'leds;
        }
        *led = background;
    }
}
