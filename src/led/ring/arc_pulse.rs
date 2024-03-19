use super::{render_lines, Animation};
use crate::{
    led::{AnimationState, RingFrame},
    mcu::main::Rgb,
};
use std::{any::Any, f64::consts::PI};

const SPEED: f64 = PI * 2.0 / 3.0; // 3 seconds per wave
const ARC_MIN_RAD: f64 = PI / 180.0 * 15.0; // 15 degrees
const ARC_MAX_RAD: f64 = PI / 180.0 * 30.0; // 30 degrees

/// Pulsing top arc.
pub struct ArcPulse {
    color: Rgb,
    pub(crate) shape: Shape,
}

#[derive(Clone)]
pub struct Shape {
    phase: f64,
    arc_min: f64,
}

impl ArcPulse {
    /// Creates a new [`ArcPulse`].
    #[must_use]
    pub fn new(color: Rgb) -> Self {
        Self { color, shape: Shape { phase: 0.0, arc_min: 0.0 } }
    }
}

impl Animation for ArcPulse {
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
        self.shape.phase = (self.shape.phase + dt * SPEED) % (PI * 2.0);
        self.shape.arc_min = (self.shape.arc_min + dt * ARC_MIN_RAD * 2.0).min(ARC_MIN_RAD);
        AnimationState::Running
    }

    fn transition_from(&mut self, superseded: &dyn Any) {
        if let Some(other) = superseded.downcast_ref::<ArcPulse>() {
            self.shape = other.shape.clone();
        }
    }
}

impl Shape {
    #[allow(clippy::cast_precision_loss)]
    pub fn render(&self, frame: &mut RingFrame, color: Rgb) {
        let arc = self.arc_length();
        let start = PI - arc / 2.0;
        let end = PI + arc / 2.0;
        render_lines(frame, Rgb::OFF, color, &[start..end]);
    }

    pub fn arc_length(&self) -> f64 {
        self.arc_min + (1.0 - self.phase.cos()) / 2.0 * (ARC_MAX_RAD - self.arc_min)
    }
}
