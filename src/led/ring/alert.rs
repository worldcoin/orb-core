use super::{
    arc_dash, arc_pulse, progress, segmented, slider, Animation, ArcDash, ArcPulse, Progress,
    Segmented, Slider, Spinner,
};
use crate::{
    led::{AnimationState, RingFrame},
    mcu::main::Rgb,
};
use std::any::Any;

/// Pulsing following a `pattern`.
pub struct Alert {
    color: Rgb,
    pattern: Vec<f64>,
    phase: f64,
    wrapped: Option<Wrapped>,
    shape: Option<Shape>,
}

enum Wrapped {
    Slider(Slider),
}

#[allow(clippy::enum_variant_names)]
enum Shape {
    ArcPulse(arc_pulse::Shape),
    Slider(slider::Shape),
    Progress(progress::Shape),
    ArcDash(arc_dash::Shape),
    Segmented(segmented::Shape),
    FullCircle,
}

impl Alert {
    /// Creates a new [`Alert`].
    #[must_use]
    pub fn new(color: Rgb, pattern: Vec<f64>) -> Self {
        Self { color, pattern, phase: 0.0, wrapped: None, shape: None }
    }
}

impl Animation for Alert {
    type Frame = RingFrame;

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn animate(&mut self, frame: &mut RingFrame, dt: f64, idle: bool) -> AnimationState {
        if let Some(wrapped) = &mut self.wrapped {
            match wrapped {
                Wrapped::Slider(slider) => {
                    if slider.animate(frame, dt, idle).is_running() {
                        return AnimationState::Running;
                    }
                    self.shape = Some(Shape::Slider(slider.shape.clone()));
                    self.wrapped = None;
                }
            }
        }
        if !idle {
            if let Some(shape) = &self.shape {
                let mut color = Rgb::OFF;
                let mut time_acc = 0.0;
                for (i, &time) in self.pattern.iter().enumerate() {
                    time_acc += time;
                    if self.phase < time_acc {
                        color = if i % 2 == 0 { self.color } else { Rgb::OFF };
                        break;
                    }
                }
                match shape {
                    Shape::ArcPulse(shape) => shape.render(frame, color),
                    Shape::Slider(shape) => shape.render(frame, color),
                    Shape::Progress(shape) => shape.render(frame, color),
                    Shape::ArcDash(shape) => shape.render(frame, color),
                    Shape::Segmented(shape) => shape.render(frame, color),
                    Shape::FullCircle => {
                        for led in frame {
                            *led = color;
                        }
                    }
                }
            }
        }
        self.phase += dt;

        if self.phase < self.pattern.iter().sum::<f64>() {
            AnimationState::Running
        } else {
            AnimationState::Finished
        }
    }

    fn transition_from(&mut self, superseded: &dyn Any) {
        if let Some(other) = superseded.downcast_ref::<ArcPulse>() {
            self.shape = Some(Shape::ArcPulse(other.shape.clone()));
        } else if let Some(other) = superseded.downcast_ref::<Slider>() {
            let mut other = other.clone();
            other.stop();
            self.wrapped = Some(Wrapped::Slider(other));
        } else if let Some(other) = superseded.downcast_ref::<Progress>() {
            self.shape = Some(Shape::Progress(other.shape.clone()));
        } else if let Some(other) = superseded.downcast_ref::<ArcDash>() {
            self.shape = Some(Shape::ArcDash(other.shape.clone()));
        } else if let Some(other) = superseded.downcast_ref::<Segmented>() {
            self.shape = Some(Shape::Segmented(other.shape.clone()));
        } else if superseded.is::<Spinner>() {
            self.shape = Some(Shape::FullCircle);
        }
    }
}
