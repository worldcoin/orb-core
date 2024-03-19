//! Movable mirrors agent.

use crate::{
    calibration, calibration::Calibration, ext::mpsc::SenderExt, port, port::Port,
    time_series::TimeSeries,
};
use async_trait::async_trait;
use eyre::Result;
use futures::{channel::oneshot, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    f64::consts::PI,
    mem::take,
    ops::{Add, Sub},
};

const HORIZONTAL_MIN: f64 = 26.0;
const HORIZONTAL_NEUTRAL: f64 = 45.0;
const HORIZONTAL_MAX: f64 = 64.0;
// TODO: Maybe decrease this to 30 and add voices to tell you to go down, up, left, right if you
// are not in the reachable range for the orb.
const VERTICAL_MIN: f64 = -35.0;
const VERTICAL_NEUTRAL: f64 = 0.0;
const VERTICAL_MAX: f64 = 35.0;

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
    /// Horizontal angle for the mirror parameter history.
    pub horizontal: TimeSeries<f64>,
    /// Vertical angle for the mirror parameter history.
    pub vertical: TimeSeries<f64>,
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
    /// Horizontal angle for the mirror.
    pub horizontal: f64,
    /// Vertical angle for the mirror.
    pub vertical: f64,
}

impl Port for Actuator {
    type Input = Command;
    type Output = (u32, i32);

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Actuator {
    const NAME: &'static str = "mirror";
}

#[async_trait]
impl super::AgentTask for Actuator {
    async fn run(self, mut port: port::Inner<Self>) -> Result<()> {
        let mut calibration = Point::from(&self.calibration.mirror);
        let mut log = Log::default();
        while let Some(command) = port.rx.next().await {
            let chain = command.chain_fn();
            match command.value {
                Command::SetPoint(point) => {
                    port.send_now(chain(convert_mirror_point(point + calibration)))?;
                    log.horizontal.push(point.horizontal);
                    log.vertical.push(point.vertical);
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

impl port::Outer<Actuator> {
    /// Takes the configuration history log.
    pub async fn take_log(&mut self) -> Result<Log> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Command::TakeLog(tx))).await?;
        Ok(rx.await?)
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            horizontal: TimeSeries::builder().limit(1_000_000).build(),
            vertical: TimeSeries::builder().limit(1_000_000).build(),
        }
    }
}

impl Point {
    /// Returns the mirror neutral point.
    #[must_use]
    pub fn neutral() -> Self {
        Self { horizontal: HORIZONTAL_NEUTRAL, vertical: VERTICAL_NEUTRAL }
    }
}

impl From<&calibration::Mirror> for Point {
    fn from(calibration: &calibration::Mirror) -> Self {
        Self { horizontal: calibration.horizontal_offset, vertical: calibration.vertical_offset }
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            horizontal: self.horizontal + other.horizontal,
            vertical: self.vertical + other.vertical,
        }
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            horizontal: self.horizontal - other.horizontal,
            vertical: self.vertical - other.vertical,
        }
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn convert_mirror_point(point: Point) -> (u32, i32) {
    let (horizontal, vertical) = calc_servo_angles(point);
    tracing::trace!("Mirror coordinates are: horizontal ({horizontal}), vertical ({vertical})");
    if !(HORIZONTAL_MIN..=HORIZONTAL_MAX).contains(&horizontal)
        || !(VERTICAL_MIN..=VERTICAL_MAX).contains(&vertical)
    {
        tracing::warn!(
            "Mirror coordinates out of range: {HORIZONTAL_MIN} <= {horizontal} <= \
             {HORIZONTAL_MAX}, {VERTICAL_MIN} <= {vertical} <= {VERTICAL_MAX}"
        );
    }
    (
        (horizontal.clamp(HORIZONTAL_MIN, HORIZONTAL_MAX) * 1000.0).round() as _,
        (vertical.clamp(VERTICAL_MIN, VERTICAL_MAX) * 1000.0).round() as _,
    )
}

fn calc_servo_angles(point: Point) -> (f64, f64) {
    let Point { horizontal, vertical } = point;
    let (theta, gamma) = angles_on_motor_planes(90.0 - horizontal, -vertical);
    (horizontal_servo_angle(theta), vertical_servo_angle(gamma))
}

fn horizontal_servo_angle(theta_angle: f64) -> f64 {
    const A1: f64 = 18.385;
    const A2: f64 = 4.243;
    const A3: f64 = 13.741;
    const A4: f64 = 19.307;
    const R1: f64 = 7.0;
    const R2: f64 = 14.0;
    let g1: f64 = to_rad(90.0); // FIXME should be const

    let b1 = (A1.powi(2) + A2.powi(2)).sqrt();
    let h1 = A2.atan2(A1);
    let h2 = h1 + to_rad(theta_angle);
    let b2 = b1 * h2.cos();
    let b3 = b1 * h2.sin();
    let b4 = A3 - b2;
    let b5 = A4 - b3;
    let h3 = b4.atan2(b5);
    let b6 = (b4.powi(2) + b5.powi(2)).sqrt();
    let h4 = ((R1.powi(2) + b6.powi(2) - R2.powi(2)) / (2.0 * R1 * b6)).acos();

    to_degree(g1 - (h3 + h4))
}

fn vertical_servo_angle(phi_angle: f64) -> f64 {
    const R1: f64 = 15.0;
    const R2: f64 = 7.0;
    const A1: f64 = 22.00;
    const A2: f64 = 4.3;
    const R3: f64 = 23.534;
    let g1: f64 = to_rad(90.0); // FIXME should be const

    let b1 = R1 * (to_rad(90.0) - to_rad(phi_angle)).cos();
    let b2 = R2 * (to_rad(90.0) - to_rad(phi_angle)).sin();
    let b3 = b1 + A1;
    let b4 = b2 - A2;
    let b5 = (b3.powi(2) + b4.powi(2)).sqrt();
    let h1 = b4.atan2(b2);
    let h2 = ((R2.powi(2) + b5.powi(2) - R3.powi(2)) / (2.0 * R2 * b5)).acos();

    to_degree(to_rad(180.0) - (h1 + h2 + g1))
}

fn angles_on_motor_planes(horizontal: f64, vertical: f64) -> (f64, f64) {
    let mut theta = -(horizontal - 45.0);
    let mut phi = -vertical;

    // TODO: Delete and fix for real
    theta = -theta;
    phi = -phi;

    let gamma = to_degree((to_rad(phi).tan() * to_rad(theta).acos()).atan());
    (theta, gamma)
}

fn to_rad(degrees: f64) -> f64 {
    (degrees / 180.0) * PI
}

#[allow(clippy::cast_possible_truncation)]
fn to_degree(rad: f64) -> f64 {
    (rad * 180.0) / PI
}
