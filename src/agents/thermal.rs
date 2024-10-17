//! Thermal Agent.

use crate::{
    consts::{MAXIMUM_FAN_SPEED, MINIMUM_FAN_SPEED},
    pid::{InstantTimer, Pid, Timer},
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    ops::RangeInclusive,
    time::{Duration, SystemTime},
};

// Fan speed range.
const FAN_SPEED_RANGE: RangeInclusive<f32> = MINIMUM_FAN_SPEED..=MAXIMUM_FAN_SPEED;

// Temperature offset from critical temperature in degree Celsius to apply fan control.
const DEFAULT_TARGET_TEMPERATURE_OFFSET: f64 = 20.0;
const DEFAULT_HIGH_TEMPERATURE_OFFSET: f64 = 10.0;

// PID parameters. Use only proportional: we might never reach the target value, so do not add up error in the integral part
const PID_PROPORTIONAL: f64 = -(*FAN_SPEED_RANGE.end() as f64 / DEFAULT_TARGET_TEMPERATURE_OFFSET);
/// Removing the I and D term for now for simplicity as requested by Hardware team, keeping the values
/// here for the future
// const PID_INTEGRAL: f64 = -0.005;
// const PID_DERIVATIVE: f64 = -0.025;
// const PID_FILTER: f64 = 40.0;

// UX specific constant
const TEMPERATURE_MOVING_AVERAGE_WINDOW: Duration = Duration::from_secs(90);

// Critical temperatures for components.
const JETSON_CPU_CRITICAL: f64 = 85.0;
const JETSON_GPU_CRITICAL: f64 = 85.0;
const MAIN_MCU_CRITICAL: f64 = 70.0;
const SEC_MCU_CRITICAL: f64 = 85.0;
const LIQUID_LENS_CRITICAL: f64 = 85.0;
const FRONT_UNIT_CRITICAL: f64 = 75.0;
const BACKUP_BATTERY_CRITICAL: f64 = 75.0;
const MAINBOARD_CRITICAL: f64 = 85.0;
/// Pausing the use of accelerometer data as Tobi said that we should not use them for now
// const MAIN_ACCELEROMETER_CRITICAL: f64 = 80.0;
// const SEC_ACCELEROMETER_CRITICAL: f64 = 80.0;

/// The margin before which a fan update is actually applied
const FAN_SPEED_UPDATE_MARGIN: f32 = 0.01;

/// A Thermal State.
/// Contains `fan_speed` and `temperature_level` for a component.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThermalState {
    fan_speed: f32,
    temperature_level: TemperatureLevel,
}

impl Default for ThermalState {
    fn default() -> Self {
        Self { fan_speed: MINIMUM_FAN_SPEED, temperature_level: TemperatureLevel::default() }
    }
}

/// Temperature level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TemperatureLevel {
    /// Normal temperature level.
    #[default]
    Normal = 0,
    /// High temperature level.
    High = 1,
    /// Critical temperature level.
    Critical = 2,
}

/// Thermal Agent struct.
///
/// See [the module-level documentation](self) for details.
#[derive(Debug)]
pub struct Agent {
    system_thermal_state: ThermalState,
    cpu_thermal_controller: ThermalController,
    gpu_thermal_controller: ThermalController,
    main_mcu_thermal_controller: ThermalController,
    sec_mcu_thermal_controller: ThermalController,
    liquid_lens_thermal_controller: ThermalController,
    front_unit_thermal_controller: ThermalController,
    // main_accelerometer_thermal_controller: ThermalController,
    // sec_accelerometer_thermal_controller: ThermalController,
    backup_battery_thermal_controller: ThermalController,
    mainboard_thermal_controller: ThermalController,
}

impl Default for Agent {
    fn default() -> Self {
        Self {
            system_thermal_state: ThermalState::default(),
            cpu_thermal_controller: ThermalController::new("cpu", JETSON_CPU_CRITICAL, None, None),
            gpu_thermal_controller: ThermalController::new("gpu", JETSON_GPU_CRITICAL, None, None),
            main_mcu_thermal_controller: ThermalController::new(
                "main mcu",
                MAIN_MCU_CRITICAL,
                None,
                None,
            ),
            sec_mcu_thermal_controller: ThermalController::new(
                "security mcu",
                SEC_MCU_CRITICAL,
                None,
                None,
            ),
            liquid_lens_thermal_controller: ThermalController::new(
                "liquid lens",
                LIQUID_LENS_CRITICAL,
                None,
                None,
            ),
            front_unit_thermal_controller: ThermalController::new(
                "front unit",
                FRONT_UNIT_CRITICAL,
                None,
                None,
            ),
            // main_accelerometer_thermal_controller: ThermalController::new(
            //     "main accelerometer",
            //     MAIN_ACCELEROMETER_CRITICAL,
            //     None,
            //     None,
            // ),
            // sec_accelerometer_thermal_controller: ThermalController::new(
            //     "security accelerometer",
            //     SEC_ACCELEROMETER_CRITICAL,
            //     None,
            //     None,
            // ),
            backup_battery_thermal_controller: ThermalController::new(
                "backup battery",
                BACKUP_BATTERY_CRITICAL,
                None,
                None,
            ),
            mainboard_thermal_controller: ThermalController::new(
                "mainboard",
                MAINBOARD_CRITICAL,
                None,
                None,
            ),
        }
    }
}

/// Input temperatures for the Thermal Agent.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum Input {
    JetsonCpu(i16),
    JetsonGpu(i16),
    MainMcu(i32),
    SecurityMcu(i32),
    LiquidLens(i32),
    FrontUnit(i32),
    MainAccelerometer(i32),
    SecurityAccelerometer(i32),
    BackupBattery(i32),
    Mainboard(i32),
}

/// Thermal Agent output.
#[derive(Debug)]
pub enum Output {
    /// New Fan speed.
    FanSpeed(f32),
    /// New `TemperatureLevel`
    TemperatureLevel(TemperatureLevel),
}

impl Port for Agent {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 10;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "thermal";
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    async fn run(mut self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        // set default fan speed and temperature level on initialization
        port.send(port::Output::new(Output::FanSpeed(self.system_thermal_state.fan_speed))).await?;
        port.send(port::Output::new(Output::TemperatureLevel(
            self.system_thermal_state.temperature_level,
        )))
        .await?;
        while let Some(input) = port.next().await {
            match input.value {
                Input::JetsonCpu(temperature) => {
                    self.cpu_thermal_controller.update(f64::from(temperature));
                }
                Input::JetsonGpu(temperature) => {
                    self.gpu_thermal_controller.update(f64::from(temperature));
                }
                Input::MainMcu(temperature) => {
                    self.main_mcu_thermal_controller.update(f64::from(temperature));
                }
                Input::SecurityMcu(temperature) => {
                    self.sec_mcu_thermal_controller.update(f64::from(temperature));
                }
                Input::LiquidLens(temperature) => {
                    self.liquid_lens_thermal_controller.update(f64::from(temperature));
                }
                Input::FrontUnit(temperature) => {
                    self.front_unit_thermal_controller.update(f64::from(temperature));
                }
                Input::MainAccelerometer(_temperature) => {
                    // self.main_accelerometer_thermal_controller.update(f64::from(temperature));
                }
                Input::SecurityAccelerometer(_temperature) => {
                    // self.sec_accelerometer_thermal_controller.update(f64::from(temperature));
                }
                Input::BackupBattery(temperature) => {
                    self.backup_battery_thermal_controller.update(f64::from(temperature));
                }
                Input::Mainboard(temperature) => {
                    self.mainboard_thermal_controller.update(f64::from(temperature));
                }
            }
            let current_highest = get_highest_thermal_state(vec![
                self.cpu_thermal_controller.thermal_state,
                self.gpu_thermal_controller.thermal_state,
                self.main_mcu_thermal_controller.thermal_state,
                self.sec_mcu_thermal_controller.thermal_state,
                self.liquid_lens_thermal_controller.thermal_state,
                self.front_unit_thermal_controller.thermal_state,
                // self.main_accelerometer_thermal_controller.thermal_state,
                // self.sec_accelerometer_thermal_controller.thermal_state,
                self.backup_battery_thermal_controller.thermal_state,
                self.mainboard_thermal_controller.thermal_state,
            ]);
            if (current_highest.fan_speed - self.system_thermal_state.fan_speed).abs()
                > FAN_SPEED_UPDATE_MARGIN
            {
                self.system_thermal_state.fan_speed = current_highest.fan_speed;
                port.send(port::Output::new(Output::FanSpeed(self.system_thermal_state.fan_speed)))
                    .await?;
            }
            if current_highest.temperature_level != self.system_thermal_state.temperature_level {
                self.system_thermal_state.temperature_level = current_highest.temperature_level;
                if port
                    .send(port::Output::new(Output::TemperatureLevel(
                        self.system_thermal_state.temperature_level,
                    )))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Returns the highest `ThermalState` from the `thermal_states`.
fn get_highest_thermal_state(thermal_states: Vec<ThermalState>) -> ThermalState {
    let mut highest_fan_speed = MINIMUM_FAN_SPEED;
    let mut highest_temperature_level = TemperatureLevel::Normal;
    for thermal_state in thermal_states {
        if thermal_state.fan_speed > highest_fan_speed {
            highest_fan_speed = thermal_state.fan_speed;
        }
        if thermal_state.temperature_level as u8 > highest_temperature_level as u8 {
            highest_temperature_level = thermal_state.temperature_level;
        }
    }
    ThermalState { fan_speed: highest_fan_speed, temperature_level: highest_temperature_level }
}

/// Controls the thermals for component.
#[derive(Debug, Clone, Copy)]
struct TemperatureDataPoint {
    value: f64,
    timestamp: SystemTime,
}

/// Controls the thermals for component.
#[derive(Debug)]
#[allow(clippy::module_name_repetitions, dead_code)]
pub struct ThermalController {
    name: String,
    pid: Pid,
    timer: InstantTimer,
    target_temperature: f64,
    critical_temperature: f64,
    high_temperature: f64,
    temperature_history: Vec<TemperatureDataPoint>,
    /// Current `ThermalState`.
    pub thermal_state: ThermalState,
}

impl ThermalController {
    /// Creates a new `ThermalController` with the default initial state.
    fn new(
        name: &str,
        critical_temperature: f64,
        target_temperature: Option<f64>,
        high_temperature: Option<f64>,
    ) -> Self {
        Self {
            name: name.to_string(),
            pid: Pid::default().with_proportional(PID_PROPORTIONAL),
            timer: InstantTimer::default(),
            target_temperature: if let Some(target_temperature) = target_temperature {
                target_temperature
            } else {
                critical_temperature - DEFAULT_TARGET_TEMPERATURE_OFFSET
            },
            critical_temperature,
            high_temperature: if let Some(high_temperature) = high_temperature {
                high_temperature
            } else {
                critical_temperature - DEFAULT_HIGH_TEMPERATURE_OFFSET
            },
            temperature_history: Vec::new(),
            thermal_state: ThermalState::default(),
        }
    }
}

impl ThermalController {
    /// Updates the controller with the current temperature and target temperature.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    pub fn update(&mut self, current_temperature: f64) {
        self.temperature_history.push(TemperatureDataPoint {
            value: current_temperature,
            timestamp: SystemTime::now(),
        });
        self.temperature_history = self
            .temperature_history
            .clone()
            .into_iter()
            .filter(|temp| {
                SystemTime::now().duration_since(temp.timestamp).unwrap_or(Duration::from_secs(0))
                    < TEMPERATURE_MOVING_AVERAGE_WINDOW
            })
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let moving_average_temperature: f64 =
            self.temperature_history.iter().map(|t| t.value).sum::<f64>()
                / self.temperature_history.len() as f64;

        if moving_average_temperature < self.high_temperature {
            self.thermal_state.temperature_level = TemperatureLevel::Normal;
        } else if moving_average_temperature < self.critical_temperature {
            self.thermal_state.temperature_level = TemperatureLevel::High;
        } else {
            self.thermal_state.temperature_level = TemperatureLevel::Critical;
            self.thermal_state.fan_speed = *FAN_SPEED_RANGE.end();
        }
        let dt = self.timer.get_dt().unwrap_or(0.0);
        let pid_result: f32 =
            self.pid.advance(self.target_temperature, moving_average_temperature, dt) as f32;
        let new_fan_speed = pid_result.clamp(*FAN_SPEED_RANGE.start(), *FAN_SPEED_RANGE.end());

        self.thermal_state.fan_speed = new_fan_speed;
        // tracing::debug!(
        //     "🌡  {}: current temperature: {}, moving avg: {}, target temperature: {}, PID: {}, fan \
        //      speed: {}",
        //     self.name,
        //     current_temperature,
        //     moving_average_temperature,
        //     self.target_temperature,
        //     pid_result,
        //     self.thermal_state.fan_speed
        // );
    }
}
