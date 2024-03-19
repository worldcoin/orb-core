use super::Animation;
use crate::{
    led::{AnimationState, RingFrame},
    mcu::main::Rgb,
};
use std::any::Any;

/// Idle / not animated ring = all LEDs in one color
/// by default, all off
pub struct Idle {
    color: Rgb,
    max_time: Option<f64>,
}

impl Idle {
    /// Create idle ring
    #[must_use]
    pub fn new(color: Option<Rgb>, max_time: Option<f64>) -> Self {
        Self { color: color.unwrap_or(Rgb::OFF), max_time }
    }
}

impl Default for Idle {
    fn default() -> Self {
        Self { color: Rgb::OFF, max_time: None }
    }
}

impl Animation for Idle {
    type Frame = RingFrame;

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    #[allow(clippy::cast_precision_loss)]
    fn animate(&mut self, frame: &mut RingFrame, dt: f64, idle: bool) -> AnimationState {
        if !idle {
            for led in frame {
                *led = self.color;
            }
            if let Some(max_time) = self.max_time {
                if max_time <= 0.0 {
                    return AnimationState::Finished;
                }
                self.max_time = Some(max_time - dt);
            }
        }
        AnimationState::Running
    }

    fn transition_from(&mut self, _superseded: &dyn Any) {}
}
