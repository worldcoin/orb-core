//! Distance measurement agent.

use crate::{
    agents::python,
    consts::{IRIS_SHARPNESS_MIN, IR_FOCUS_DISTANCE, IR_FOCUS_RANGE, IR_FOCUS_RANGE_SMALL},
    dd_incr,
    time_series::TimeSeries,
    ui,
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::{channel::oneshot, prelude::*};
use std::time::{Duration, SystemTime};

/// User distance estimation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Unknown/uninitialized state.
    Unknown,
    /// User is in range.
    InRange,
    /// User is too close.
    TooClose,
    /// User is too far.
    TooFar,
}

/// Distance measurement agent.
///
/// See [the module-level documentation](self) for details.
pub struct Agent {
    /// UI engine.
    pub ui: Box<dyn ui::Engine>,
}

/// Agent input.
#[derive(Debug)]
pub enum Input {
    /// IR Net estimation.
    IrNetEstimate(python::ir_net::EstimateOutput),
    /// RGB Net estimation.
    RgbNetEstimate(python::rgb_net::EstimateOutput),
    /// Resets the internal state of the agent.
    Reset(oneshot::Sender<Log>),
}

/// User distance history.
#[derive(Debug)]
pub struct Log {
    /// User distance time-series.
    pub user_distance: TimeSeries<f64>,
}

impl Default for Log {
    fn default() -> Self {
        Self { user_distance: TimeSeries::builder().limit(1_000_000).build() }
    }
}

/// Resets the capturing and returns configuration history log.
pub async fn reset(port: &mut port::Outer<Agent>) -> Result<Log> {
    let (tx, rx) = oneshot::channel();
    port.send_unjam(port::Input::new(Input::Reset(tx))).await?;
    Ok(rx.await?)
}

impl Port for Agent {
    type Input = Input;
    type Output = Status;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "distance";
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        'reset: loop {
            let mut focus_range = IR_FOCUS_RANGE_SMALL;
            let mut sharp_iris_detected = false;
            let mut status;
            let mut user_came_in_range = false;
            let mut rgb_net_first_distance_date = None;
            let mut log = Log::default();
            loop {
                match port.next().await {
                    Some(input) => match input.value {
                        Input::IrNetEstimate(ref ir_net_estimate) => {
                            let python::ir_net::EstimateOutput { sharpness, .. } = *ir_net_estimate;
                            if sharpness > IRIS_SHARPNESS_MIN && !sharp_iris_detected {
                                dd_incr!(
                                    "main.count.signup.during.biometric_capture.\
                                     sharp_iris_detected"
                                );
                                sharp_iris_detected = true;
                            }
                        }
                        Input::RgbNetEstimate(rgb_net_estimate) => {
                            if rgb_net_first_distance_date.is_none() {
                                rgb_net_first_distance_date = Some(SystemTime::now());
                            }
                            let Some(user_distance) = rgb_net_estimate
                                .primary()
                                .map(python::rgb_net::EstimatePredictionOutput::user_distance)
                            else {
                                continue;
                            };
                            log.user_distance.push(user_distance);
                            status = if focus_range.contains(&user_distance) {
                                focus_range = IR_FOCUS_RANGE;
                                user_came_in_range = true;
                                rgb_net_first_distance_date = Some(SystemTime::now());
                                self.ui.biometric_capture_distance(true);
                                Status::InRange
                            } else {
                                // show "user not in range" only if user was in range before
                                let time_out_of_range = rgb_net_first_distance_date
                                    .unwrap_or(SystemTime::now())
                                    .elapsed()
                                    .unwrap_or(Duration::from_secs(0));
                                if user_came_in_range || time_out_of_range.as_millis() > 2000 {
                                    self.ui.biometric_capture_distance(false);
                                    user_came_in_range = false;
                                }
                                focus_range = IR_FOCUS_RANGE_SMALL;
                                if user_distance < IR_FOCUS_DISTANCE {
                                    Status::TooClose
                                } else {
                                    Status::TooFar
                                }
                            };
                            port.send(port::Output::new(status)).await?;
                        }
                        Input::Reset(log_tx) => {
                            tracing::debug!("RESETTING DISTANCE AGENT");
                            #[allow(let_underscore_drop)]
                            let _ = log_tx.send(log);
                            continue 'reset;
                        }
                    },
                    None => return Ok(()),
                }
            }
        }
    }
}
