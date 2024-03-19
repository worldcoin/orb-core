use super::{render_lines, Animation};
use crate::{
    led::{AnimationState, RingFrame},
    mcu::main::Rgb,
};
use std::{any::Any, f64::consts::PI};

/// Progress growing from the center of the left and the right halves.
pub struct FakeProgress {
    color: Rgb,
    pub(crate) shape: Shape,
}

#[derive(Clone)]
pub struct Shape {
    duration: f64,
    phase: f64,
}

impl FakeProgress {
    /// Creates a new [`FakeProgress`].
    #[must_use]
    pub fn new(duration: f64, color: Rgb) -> Self {
        Self { color, shape: Shape { duration, phase: 0.0 } }
    }
}

impl Animation for FakeProgress {
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
        self.shape.phase += dt;
        if self.shape.phase < self.shape.duration {
            AnimationState::Running
        } else {
            AnimationState::Finished
        }
    }
}

impl Shape {
    #[allow(clippy::cast_precision_loss)]
    pub fn render(&self, frame: &mut RingFrame, color: Rgb) {
        let progress = self.phase / self.duration;
        let angle = PI * progress;
        let ranges = [PI - angle..PI, PI..PI + angle];
        render_lines(frame, Rgb::OFF, color, &ranges);
    }
}
