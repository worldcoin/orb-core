use std::{any::Any, f64::consts::PI};

use crate::{
    led::{ring::LIGHT_BLEEDING_OFFSET_RAD, AnimationState, RingFrame},
    mcu::main::Rgb,
};

use super::{render_lines, Animation, ArcPulse};

pub const ARC_LENGTH: f64 = PI / 180.0 * 15.0; // 15 degrees

const RC: f64 = 0.5;
const COMPLETE_POINT: f64 = 0.95;
const COMPLETE_TIME: f64 = 1.5;
const PULSE_SPEED: f64 = PI * 2.0 / 3.0 /* seconds per wave */;
const PULSE_AMPLITUDE_PERCENT: f64 = 0.05;

/// Sliding both sides top down, also known as the "waterfall" effect.
/// Once the progress reaches COMPLETE_POINT, the animation will complete with a static
/// color for COMPLETE_TIME seconds.
#[derive(Clone)]
pub struct Slider {
    color: Rgb,
    progress: f64,
    pub(crate) shape: Shape,
    complete_time: f64,
}

#[derive(Clone)]
pub struct Shape {
    progress: f64,
    pulse_phase: Option<f64>,
}

impl Slider {
    /// Creates a new [`Slider`].
    #[must_use]
    pub fn new(progress: f64, color: Rgb) -> Self {
        Self {
            color,
            progress,
            complete_time: COMPLETE_TIME,
            shape: Shape { progress, pulse_phase: None },
        }
    }

    /// Sets the progress value for the slider.
    pub fn set_progress(&mut self, progress: f64, clip_before_completion: bool) {
        let upper_bound = if clip_before_completion { COMPLETE_POINT - f64::EPSILON } else { 1.0 };
        self.progress = progress.min(upper_bound);
    }

    /// Pulses remaining progress.
    #[must_use]
    pub fn pulse_remaining(mut self) -> Self {
        self.shape.pulse_phase = Some(0.0);
        self
    }
}

impl Animation for Slider {
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

        if let Some(pulse_phase) = &mut self.shape.pulse_phase {
            *pulse_phase = (*pulse_phase + dt * PULSE_SPEED) % (PI * 2.0);
        }
        if self.shape.progress < COMPLETE_POINT {
            self.shape.progress =
                self.shape.progress + (dt / (RC + dt)) * (self.progress - self.shape.progress);
            AnimationState::Running
        } else {
            self.shape.progress = 1.0;
            self.complete_time -= dt;
            if self.complete_time > 0.0 {
                AnimationState::Running
            } else {
                AnimationState::Finished
            }
        }
    }

    fn transition_from(&mut self, superseded: &dyn Any) {
        if let Some(other) = superseded.downcast_ref::<ArcPulse>() {
            self.shape.progress = (other.shape.arc_length() / 2.0 - ARC_LENGTH) / (PI - ARC_LENGTH);
        }
    }
}

impl Shape {
    #[allow(clippy::cast_precision_loss)]
    pub fn render(&self, frame: &mut RingFrame, color: Rgb) {
        let mut progress = self.progress.clamp(0.0, 1.0);
        if progress
            < (COMPLETE_POINT - PULSE_AMPLITUDE_PERCENT - LIGHT_BLEEDING_OFFSET_RAD / (2.0 * PI))
        {
            if let Some(phase) = self.pulse_phase {
                progress += phase.sin() / 2.0 * PULSE_AMPLITUDE_PERCENT;
            }
        }
        let angle = (PI - ARC_LENGTH) * progress + ARC_LENGTH;
        let ranges = [PI - angle..PI, PI..PI + angle];
        render_lines(frame, Rgb::OFF, color, &ranges);
    }
}
