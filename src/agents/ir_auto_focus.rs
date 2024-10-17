//! Auto-focus for IR camera.

use crate::{
    agents::{camera, camera::Frame, python},
    consts::{AUTOFOCUS_MAX, AUTOFOCUS_MIN, IR_FOCUS_RANGE},
    dsp::Lagging,
    pid::{derivative::LowPassFilter, InstantTimer, Pid, Timer},
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::prelude::*;
use ndarray::prelude::*;
use std::{ops::RangeInclusive, time::Instant};

const FOCUS_RANGE: RangeInclusive<i16> = AUTOFOCUS_MIN..=AUTOFOCUS_MAX;

/// Proportional PID parameter.
pub const PID_PROPORTIONAL: f64 = 2.35;

/// Integral PID parameter.
pub const PID_INTEGRAL: f64 = 1.2;

// DerivedSignal parameters.
const DERIVED_SIGNAL_FILTER: f64 = 0.03;
const DERIVED_SIGNAL_LAG: f64 = 0.09;

// Maximal Δfocus/Δt. Limits the maximum rate of focus change.
const MAX_DF_DT: f64 = 300.0;

// How much difference from the absolute peak is considered fine.
const PEAK_TOLERANCE: f64 = 0.1;

// Minimum absolute value of the derived signal to be considered for focus
// direction flip.
const FLIP_THRESHOLD: f64 = 0.033;

// Default minimal viable sharpness.
const DEFAULT_MIN_SHARPNESS: f64 = 1.2;

// Search range in mm around estimated distance to search for sharpest iris
// image.
const DISTANCE_SEARCH_RANGE: f64 = 130.0;

// Parameters of linear relationship between spatial distance and focus setting
// (m, t)
// TODO: Replace with actual values
const DISTANCE_TO_FOCUS_PARAMS: (f64, f64) = (-1.603_612_87, 462.536_655);

/// Auto-focus for IR camera.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Debug)]
pub struct Agent;

/// Agent input.
#[derive(Debug)]
pub enum Input {
    /// Sharpness score from IR Net estimate.
    Sharpness(f64),
    /// IR camera frame for internal sharpness score calculation.
    Frame(camera::ir::Frame),
    /// User distance from RGB Net estimate.
    UserDistance(f64),
    /// Set minimal viable sharpness.
    SetMinSharpness(f64),
    /// Resets the internal state of the agent.
    Reset,
}

impl Port for Agent {
    type Input = Input;
    type Output = i16;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "ir-auto-focus";
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        'reset: loop {
            let start_timestamp = Instant::now();
            let mut update_counter: u64 = 0;
            let mut timer = InstantTimer::default();
            let mut controller = LiquidLensController::new(DEFAULT_MIN_SHARPNESS);
            let mut range = FOCUS_RANGE;
            let mut sharpness;
            while let Some(input) = port.next().await {
                match input.value {
                    Input::Sharpness(new_sharpness) => {
                        sharpness = new_sharpness;
                    }
                    Input::Frame(frame) => {
                        let height = frame.height() as usize;
                        let width = frame.width() as usize;
                        let mut gx = Array::zeros((height, width));
                        let mut gy = Array::zeros((height, width));
                        for y in 1..height - 1 {
                            for x in 1..width - 1 {
                                gx[[y, x]] =
                                    frame[(y + 1) * width + x] - frame[(y - 1) * width + x];
                                gy[[y, x]] = frame[y * width + x + 1] - frame[y * width + x - 1];
                            }
                        }
                        let gnorm = (gx.mapv(|a| f64::from(a.pow(2)))
                            + gy.mapv(|a| f64::from(a.pow(2))))
                        .mapv(f64::sqrt);
                        if let Some(new_sharpness) = gnorm.mean() {
                            sharpness = new_sharpness;
                        } else {
                            continue;
                        }
                    }
                    Input::UserDistance(user_distance) => {
                        if !user_distance.is_nan() {
                            range = user_focus_range(user_distance);
                        }
                        continue;
                    }
                    Input::SetMinSharpness(min_sharpness) => {
                        controller.set_min_sharpness(min_sharpness);
                        continue;
                    }
                    Input::Reset => {
                        let capture_time = start_timestamp.elapsed().as_secs();
                        let fps = if capture_time > 0 { update_counter / capture_time } else { 0 };
                        tracing::info!("FPS of auto focus: {}", fps);
                        continue 'reset;
                    }
                }
                let dt = timer.get_dt().unwrap_or(0.0);
                let focus = controller.update(sharpness, range.clone(), dt);
                port.send(port::Output::new(focus)).await?;
                update_counter += 1;
            }
            break;
        }
        Ok(())
    }
}

impl From<&python::ir_net::EstimateOutput> for Input {
    fn from(ir_net_estimate: &python::ir_net::EstimateOutput) -> Self {
        Self::Sharpness(ir_net_estimate.sharpness)
    }
}

impl From<&python::rgb_net::EstimateOutput> for Input {
    fn from(rgb_net_estimate: &python::rgb_net::EstimateOutput) -> Self {
        Self::UserDistance(
            rgb_net_estimate
                .predictions
                .first()
                .map_or(f64::NAN, python::rgb_net::EstimatePredictionOutput::user_distance),
        )
    }
}

/// Controls the liquid lens to stay at the most sharp focus distance.
pub struct LiquidLensController {
    min_sharpness: f64,
    pid: Pid,
    derived: DerivedSignal,
    focus_forward: bool,
    focus_curr: i16,
    sharpness_forward: bool,
    sharpness_last: f64,
    sharpness_peak: f64,
    sharpness_peak_searching: bool,
}

/// Generates the derived signal for [`LiquidLensController`].
#[derive(Default)]
pub struct DerivedSignal {
    filter: LowPassFilter,
    lagging: Lagging,
}

impl LiquidLensController {
    /// Creates a new [`LiquidLensController`].
    #[must_use]
    pub fn new(min_sharpness: f64) -> Self {
        let pid = Pid::default().with_proportional(PID_PROPORTIONAL).with_integral(PID_INTEGRAL);
        Self {
            min_sharpness,
            pid,
            derived: DerivedSignal::default(),
            focus_forward: true,
            focus_curr: *FOCUS_RANGE.start(),
            sharpness_forward: true,
            sharpness_last: 0.0,
            sharpness_peak: 0.0,
            sharpness_peak_searching: true,
        }
    }

    /// Sets the minimal viable sharpness.
    pub fn set_min_sharpness(&mut self, min_sharpness: f64) {
        self.min_sharpness = min_sharpness;
    }

    /// Updates the controller with current `sharpness` score and focus `range`
    /// limits. Returns the focus setting for the liquid lens.
    #[allow(clippy::cast_possible_truncation)]
    pub fn update(&mut self, sharpness: f64, range: RangeInclusive<i16>, dt: f64) -> i16 {
        if sharpness.is_finite() {
            self.sharpness_last = sharpness;
        }
        self.sharpness_peak = self.sharpness_peak.max(self.sharpness_last);
        if let Some(derived) = self.derived.add(self.sharpness_last, dt) {
            if derived.abs() >= FLIP_THRESHOLD && self.sharpness_forward != (derived > 0.0) {
                self.sharpness_forward = !self.sharpness_forward;
                if !self.sharpness_forward {
                    // Flip when the sharpness sets the trend to decline.
                    if self.sharpness_peak >= self.min_sharpness {
                        self.focus_forward = !self.focus_forward;
                        self.sharpness_peak_searching = false;
                        self.pid.reset();
                    }
                }
            }
        }
        let offset = if self.sharpness_peak_searching {
            MAX_DF_DT * dt
        } else if self.sharpness_peak - self.sharpness_last <= PEAK_TOLERANCE {
            // Just stop at the peak.
            self.pid.reset();
            0.0
        } else {
            self.pid
                .advance(self.sharpness_peak, self.sharpness_last, dt)
                .clamp(-MAX_DF_DT * dt, MAX_DF_DT * dt)
        };
        let offset = offset.copysign(if self.focus_forward { 1.0 } else { -1.0 }).round() as i16;
        if range.contains(&(self.focus_curr + offset)) {
            self.focus_curr += offset;
        } else {
            self.focus_curr = self.focus_curr.clamp(*range.start(), *range.end());
            if range.contains(&(self.focus_curr - offset)) {
                // Flip when reaching the range edge.
                self.focus_curr -= offset;
                self.focus_forward = !self.focus_forward;
            } else {
                // Double flip because the range has changed dramatiaclly.
                self.focus_curr += offset;
            }
            self.sharpness_peak = 0.0;
            self.sharpness_peak_searching = true;
        }
        self.focus_curr
    }
}

impl DerivedSignal {
    /// Adds a new partition of the target function. Returns the derived value.
    pub fn add(&mut self, sharpness: f64, dt: f64) -> Option<f64> {
        let filtered = self.filter.add(sharpness, dt, DERIVED_SIGNAL_FILTER);
        let lagging = self.lagging.add(filtered, dt, DERIVED_SIGNAL_LAG)?;
        Some(filtered - lagging)
    }

    /// Resets the signal.
    pub fn reset(&mut self) {
        self.filter.reset();
        self.lagging.reset();
    }
}

/// Narrows operating focus range with the distance to the user estimate.
///
/// Called on each new user distance estimation result. The user distance is the
/// spatial distance measured in mm.
#[must_use]
pub fn user_focus_range(user_distance: f64) -> RangeInclusive<i16> {
    let a = compute_theoretical_focus_setting(user_distance + DISTANCE_SEARCH_RANGE);
    let b = compute_theoretical_focus_setting(user_distance - DISTANCE_SEARCH_RANGE);
    if a < b { a..=b } else { b..=a }
}

#[allow(clippy::cast_possible_truncation)]
fn compute_theoretical_focus_setting(mut user_distance: f64) -> i16 {
    let (m, t) = DISTANCE_TO_FOCUS_PARAMS;
    user_distance = user_distance.clamp(*IR_FOCUS_RANGE.start(), *IR_FOCUS_RANGE.end());
    let focus_setting = (m * user_distance + t) as i16;
    focus_setting.clamp(*FOCUS_RANGE.start(), *FOCUS_RANGE.end())
}
