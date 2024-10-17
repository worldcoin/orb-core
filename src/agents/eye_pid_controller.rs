//! Eye PID controller agent.

use crate::{
    agents::{mirror, python},
    dd_gauge,
    pid::{InstantTimer, Pid, Timer},
    utils::RkyvNdarray,
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::prelude::*;
use ndarray::prelude::*;
use std::time::Duration;

const IRIS_DIAMETER_MM: f64 = 12.0;

// If there are no landmarks from IRNet after certain amount of time, reset the
// offset.
const RESET_DELAY: Duration = Duration::from_millis(1800);

// Maximum accepted landmarks distance.
const TRUSTED_RADIUS: f64 = 100.0;

// PID parameters.
const PID_PROPORTIONAL: f64 = 0.012;
const PID_INTEGRAL: f64 = 0.00016;
const PID_DERIVATIVE: f64 = 0.0023;
const PID_FILTER: f64 = 0.26;

// Interval during which to suppress the PID, because of the mirror is switching
// the target eye.
const SWITCH_EYE_INTERVAL: Duration = Duration::from_millis(400);

/// Eye PID controller agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Debug)]
pub struct Agent;

/// Agent input.
#[derive(Debug)]
pub enum Input {
    /// IR Net estimation.
    IrNetEstimate(python::ir_net::EstimateOutput),
    /// Notifies that the mirror is going to switch the taget eye.
    SwitchEye,
    /// Resets the internal state of the agent.
    Reset,
}

impl Port for Agent {
    type Input = Input;
    type Output = mirror::Point;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "eye-pid-controller";
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        'reset: loop {
            let mut timer = InstantTimer::default();
            let mut controller = EyeOffsetController::new(RESET_DELAY.as_secs_f64());
            let mut suppress_time = 0.0;
            while let Some(input) = port.next().await {
                match &input.value {
                    Input::IrNetEstimate(ir_net_estimate) => {
                        let python::ir_net::EstimateOutput { landmarks, sharpness, .. } =
                            ir_net_estimate;
                        let dt = timer.get_dt().unwrap_or(0.0);
                        suppress_time -= dt;
                        if suppress_time > 0.0 {
                            continue;
                        }
                        let iris_center = (*sharpness > 1.1)
                            .then_some(landmarks.as_ref())
                            .flatten()
                            .map(RkyvNdarray::<_, Ix2>::as_ndarray)
                            .and_then(iris_center_from_landmarks);
                        let (x, y) = if let Some((x, y)) = iris_center {
                            controller.update(x, y, dt)
                        } else {
                            controller.idle(dt)
                        };
                        dd_gauge!(
                            "main.gauge.signup.pid.continuous",
                            x.to_string(),
                            "type:phi_degrees"
                        );
                        dd_gauge!(
                            "main.gauge.signup.pid.continuous",
                            y.to_string(),
                            "type:theta_degrees"
                        );
                        port.send(input.chain(mirror::Point { phi_degrees: x, theta_degrees: y }))
                            .await?;
                    }
                    Input::SwitchEye => {
                        suppress_time = SWITCH_EYE_INTERVAL.as_secs_f64();
                    }
                    Input::Reset => continue 'reset,
                }
            }
            break;
        }
        Ok(())
    }
}

/// Controls the mirror offset to center to the user's iris.
pub struct EyeOffsetController {
    reset_delay: f64,
    horizontal: Pid,
    vertical: Pid,
    idle_time: f64,
    curr: (f64, f64),
}

impl EyeOffsetController {
    /// Creates a new [`EyeOffsetController`].
    #[must_use]
    pub fn new(reset_delay: f64) -> Self {
        let default_pid = Pid::default()
            .with_proportional(PID_PROPORTIONAL)
            .with_integral(PID_INTEGRAL)
            .with_derivative(PID_DERIVATIVE)
            .with_filter(PID_FILTER);
        Self {
            reset_delay,
            horizontal: default_pid.clone(),
            vertical: default_pid,
            idle_time: 0.0,
            curr: (0.0, 0.0),
        }
    }

    /// Updates the controller with predicted iris offset. Returns the mirror
    /// offset.
    pub fn update(&mut self, x: f64, y: f64, dt: f64) -> (f64, f64) {
        self.idle_time = 0.0;
        let (curr_x, curr_y) = &mut self.curr;
        *curr_x += self.horizontal.advance(0.0, x, dt);
        *curr_y -= self.vertical.advance(0.0, y, dt);
        self.curr
    }

    /// Updates the controller without predicted iris offset. Returns the mirror
    /// offset.
    pub fn idle(&mut self, dt: f64) -> (f64, f64) {
        self.idle_time += dt;
        if self.idle_time > self.reset_delay {
            self.idle_time = 0.0;
            self.horizontal.reset();
            self.vertical.reset();
            self.curr = (0.0, 0.0);
        }
        self.curr
    }
}

fn iris_center_from_landmarks(landmarks: ArrayView2<f32>) -> Option<(f64, f64)> {
    let iris_width =
        IRIS_DIAMETER_MM / f64::from((landmarks.get((4, 0))? - landmarks.get((6, 0))?).abs());
    let center = landmarks.slice(s![4..8, ..]).mean_axis(Axis(0))?;
    let center_x = (f64::from(*center.get(0)?) - 0.5) * iris_width;
    let center_y = (f64::from(*center.get(1)?) - 0.5) * iris_width;
    ((-TRUSTED_RADIUS..TRUSTED_RADIUS).contains(&center_x)
        && (-TRUSTED_RADIUS..TRUSTED_RADIUS).contains(&center_x))
    .then_some((center_x, center_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::arr2;

    #[test]
    fn test_empty_iris_center_from_landmarks() {
        let data = arr2(&[[]]);
        assert!(iris_center_from_landmarks(data.view()).is_none());
    }

    #[test]
    fn test_iris_center_from_landmarks_with_minimal_data() {
        let expect = Some((3.0, -6.0));
        let data = arr2(&[
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 0.0],
            [2.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
        ]);
        assert_eq!(iris_center_from_landmarks(data.view()), expect);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_eye_offset_controller() {
        let mut ctrl = EyeOffsetController::new(1.0);

        let res = ctrl.idle(0.5);
        assert_eq!(res, (0.0, 0.0));
        assert_eq!(ctrl.idle_time, 0.5);

        let res = ctrl.update(1.0, 1.0, 1.0);
        assert_ne!(res, (0.0, 0.0));

        let res = ctrl.idle(2.0);
        assert_eq!(res, (0.0, 0.0));
        assert_eq!(ctrl.idle_time, 0.0);
    }
}
