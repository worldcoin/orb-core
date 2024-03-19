use super::Animation;
use crate::{
    led::{AnimationState, RingFrame},
    mcu::main::{Rgb, RING_LED_COUNT},
};
use std::{any::Any, f64::consts::PI};

const PULSE_SPEED: f64 = PI * 2.0 / 3.0; // 3 seconds per pulse

/// State of one segment.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Segment {
    /// Segment is static off.
    Off,
    /// Segment is pulsing.
    Pulse,
    /// Segment is static on.
    Solid,
}

/// Segmented animation.
pub struct Segmented {
    color: Rgb,
    max_time: Option<f64>,
    pub(crate) shape: Shape,
}

#[derive(Clone)]
pub struct Shape {
    start_angle: f64,
    pattern: Vec<Segment>,
    phase: f64,
}

impl Segmented {
    /// Creates a new [`Segmented`].
    #[must_use]
    pub fn new(color: Rgb, start_angle: f64, pattern: Vec<Segment>, max_time: Option<f64>) -> Self {
        Self {
            color,
            max_time,
            shape: Shape { start_angle: start_angle % PI, pattern, phase: 0.0 },
        }
    }

    /// Returns a mutable slice of the segmented pattern.
    pub fn pattern_mut(&mut self) -> &mut [Segment] {
        &mut self.shape.pattern
    }
}

impl Animation for Segmented {
    type Frame = RingFrame;

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn animate(&mut self, frame: &mut RingFrame, dt: f64, idle: bool) -> AnimationState {
        if !idle {
            self.shape.render(frame, self.color);
        }
        self.shape.phase = (self.shape.phase + dt * PULSE_SPEED) % (PI * 2.0);
        if let Some(max_time) = &mut self.max_time {
            *max_time -= dt;
            if *max_time <= 0.0 {
                return AnimationState::Finished;
            }
        }
        AnimationState::Running
    }

    fn transition_from(&mut self, _superseded: &dyn Any) {
        self.shape.phase = 0.0;
    }
}

impl Shape {
    #[allow(clippy::cast_precision_loss, clippy::match_on_vec_items)]
    pub fn render(&self, frame: &mut RingFrame, color: Rgb) {
        let pulse_color = color * ((1.0 - self.phase.cos()) / 2.0);
        for (led_index, led) in frame.iter_mut().enumerate() {
            *led = match self.pattern[self.segment_index(led_index)] {
                Segment::Off => Rgb::OFF,
                Segment::Pulse => pulse_color,
                Segment::Solid => color,
            };
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn segment_index(&self, led_index: usize) -> usize {
        const LED: f64 = PI * 2.0 / RING_LED_COUNT as f64;
        let led_angle = (PI + self.start_angle + led_index as f64 * LED) % (PI * 2.0);
        (led_angle / (PI * 2.0 / self.pattern.len() as f64)) as usize
    }
}
