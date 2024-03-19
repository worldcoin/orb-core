//! Auto-exposure for IR camera.

use crate::{
    agents::camera,
    consts::{
        DEFAULT_IR_LED_DURATION, IR_CAMERA_DEFAULT_GAIN, IR_LED_MAX_DURATION, IR_LED_MIN_DURATION,
    },
    pid::{InstantTimer, Pid, Timer},
    port,
    port::Port,
};
use async_trait::async_trait;
use eyre::Result;
use futures::prelude::*;
use std::ops::RangeInclusive;

/// Default frame pixel mean value.
pub const DEFAULT_TARGET_MEAN: f64 = 135.0;

/// Default exposure range.
pub const DEFAULT_EXPOSURE_RANGE: RangeInclusive<u16> = IR_LED_MIN_DURATION..=IR_LED_MAX_DURATION;

// PID parameters.
const PID_PROPORTIONAL: f64 = 0.002_2;
const PID_INTEGRAL: f64 = 0.000_001;
const PID_DERIVATIVE: f64 = 0.000_01;
const PID_FILTER: f64 = 0.12;

/// Auto-exposure for IR cameras.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Debug)]
pub struct Agent;

/// Agent input.
#[derive(Debug)]
pub enum Input {
    /// IR frame.
    Frame(camera::ir::Frame),
    /// Set exposure range.
    SetExposureRange(RangeInclusive<u16>),
    /// Set target mean value.
    SetTargetMean(f64),
}

/// Auto-exposure output for IR camera.
#[derive(Debug)]
pub struct Output {
    /// Gain value for IR camera.
    pub gain: i64,
    /// Exposure value for IR camera and IR LEDs.
    pub exposure: u16,
}

impl Port for Agent {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Agent {
    const NAME: &'static str = "ir-auto-exposure";
}

#[async_trait]
impl super::AgentTask for Agent {
    async fn run(self, mut port: port::Inner<Self>) -> Result<()> {
        let mut timer = InstantTimer::default();
        #[allow(clippy::cast_precision_loss)]
        let mut controller = ExposureController::new(
            IR_CAMERA_DEFAULT_GAIN as _,
            f64::from(DEFAULT_IR_LED_DURATION),
        );
        let mut exposure_range = DEFAULT_EXPOSURE_RANGE;
        let mut target_mean = DEFAULT_TARGET_MEAN;
        while let Some(input) = port.next().await {
            match input.value {
                Input::Frame(ref frame) => {
                    let dt = timer.get_dt().unwrap_or(0.0);
                    let (gain, exposure) = controller.update(
                        f64::from(frame.mean()),
                        target_mean,
                        f64::from(*exposure_range.start())..=f64::from(*exposure_range.end()),
                        dt,
                    );
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let exposure = exposure as _;
                    #[allow(clippy::cast_possible_truncation)]
                    let gain = gain as _;
                    port.send(input.chain(Output { gain, exposure })).await?;
                }
                Input::SetExposureRange(new_exposure_range) => exposure_range = new_exposure_range,
                Input::SetTargetMean(new_target_mean) => {
                    target_mean = new_target_mean;
                    controller.pid.reset();
                    timer.reset();
                }
            }
        }
        Ok(())
    }
}

/// Controls the exposure level of IR camera.
pub struct ExposureController {
    pid: Pid,
    gain: f64,
    exposure: f64,
}

impl ExposureController {
    /// Creates a new [`ExposureController`].
    #[must_use]
    pub fn new(gain: f64, exposure: f64) -> Self {
        let pid = Pid::default()
            .with_proportional(PID_PROPORTIONAL)
            .with_integral(PID_INTEGRAL)
            .with_derivative(PID_DERIVATIVE)
            .with_filter(PID_FILTER);
        Self { pid, gain, exposure }
    }

    /// Updates the controller with the current pixel mean and target pixel mean
    /// values. Returns gain and exposure.
    // TODO implement gain
    pub fn update(
        &mut self,
        curr_mean: f64,
        target_mean: f64,
        exposure_range: RangeInclusive<f64>,
        dt: f64,
    ) -> (f64, f64) {
        self.exposure *= 1.0 + self.pid.advance(target_mean, curr_mean, dt);
        if !exposure_range.contains(&self.exposure) {
            self.exposure = self.exposure.clamp(*exposure_range.start(), *exposure_range.end());
            self.pid.reset();
        }
        (self.gain, self.exposure)
    }
}
