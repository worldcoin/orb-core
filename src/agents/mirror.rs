//! Movable mirrors agent.

use crate::{
    calibration, calibration::Calibration, ext::mpsc::SenderExt as _, time_series::TimeSeries,
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::{channel::oneshot, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    mem::take,
    ops::{Add, Sub},
};

const PHI_NEUTRAL_DEGREES: f64 = 45.0;
const THETA_NEUTRAL_DEGREES: f64 = 90.0;

/// Movable mirrors.
///
/// See [the module-level documentation](self) for details.
#[derive(Debug)]
pub struct Actuator {
    /// Calibration data.
    pub calibration: Calibration,
}

/// Configuration history.
#[derive(Debug)]
pub struct Log {
    /// Phi angle for the mirror parameter history.
    pub phi_degrees: TimeSeries<f64>,
    /// Theta angle for the mirror parameter history.
    pub theta_degrees: TimeSeries<f64>,
}

/// Actuator input.
#[derive(Debug)]
pub enum Command {
    /// Set the mirror point.
    SetPoint(Point),
    /// Update the calibration data.
    Recalibrate(Calibration),
    /// Takes the configuration log from the agent.
    TakeLog(oneshot::Sender<Log>),
}

/// One mirror point.
#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug)]
pub struct Point {
    /// Phi angle for the mirror.
    pub phi_degrees: f64,
    /// Theta angle for the mirror.
    pub theta_degrees: f64,
}

impl Port for Actuator {
    type Input = Command;
    type Output = (u32, u32);

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Actuator {
    const NAME: &'static str = "mirror";
}

impl agentwire::agent::Task for Actuator {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let mut calibration = Point::from(&self.calibration.mirror);
        let mut log = Log::default();
        while let Some(command) = port.rx.next().await {
            let chain = command.chain_fn();
            match command.value {
                Command::SetPoint(point) => {
                    let mirror_target_point = point + calibration;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let phi_millidegrees: u32 =
                        (mirror_target_point.phi_degrees * 1000.0).round() as _;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let theta_millidegrees: u32 =
                        (mirror_target_point.theta_degrees * 1000.0).round() as _;
                    port.tx.send_now(chain((phi_millidegrees, theta_millidegrees)))?;
                    log.phi_degrees.push(point.phi_degrees);
                    log.theta_degrees.push(point.theta_degrees);
                }
                Command::Recalibrate(new_calibration) => {
                    calibration = Point::from(&new_calibration.mirror);
                }
                Command::TakeLog(log_tx) => {
                    #[allow(let_underscore_drop)]
                    let _ = log_tx.send(take(&mut log));
                }
            }
        }
        Ok(())
    }
}

/// Takes the configuration history log.
pub async fn take_log(port: &mut port::Outer<Actuator>) -> Result<Log> {
    let (tx, rx) = oneshot::channel();
    port.send(port::Input::new(Command::TakeLog(tx))).await?;
    Ok(rx.await?)
}

impl Default for Log {
    fn default() -> Self {
        Self {
            phi_degrees: TimeSeries::builder().limit(1_000_000).build(),
            theta_degrees: TimeSeries::builder().limit(1_000_000).build(),
        }
    }
}

impl Point {
    /// Returns the mirror neutral point.
    #[must_use]
    pub fn neutral() -> Self {
        Self { phi_degrees: PHI_NEUTRAL_DEGREES, theta_degrees: THETA_NEUTRAL_DEGREES }
    }
}

impl From<&calibration::Mirror> for Point {
    fn from(calibration: &calibration::Mirror) -> Self {
        Self {
            phi_degrees: calibration.phi_offset_degrees,
            theta_degrees: calibration.theta_offset_degrees,
        }
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            phi_degrees: self.phi_degrees + other.phi_degrees,
            theta_degrees: self.theta_degrees + other.theta_degrees,
        }
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            phi_degrees: self.phi_degrees - other.phi_degrees,
            theta_degrees: self.theta_degrees - other.theta_degrees,
        }
    }
}
