//! Main microcontroller interface.

use super::{can::Can, serial::Serial, Interface, Mcu, ResultSender};
use crate::{
    consts::{DEFAULT_USER_LED_PULSING_PERIOD, DEFAULT_USER_LED_PULSING_SCALE},
    ext::mpsc::SenderExt,
    time_series::TimeSeries,
};
use eyre::Result;
use futures::{channel::mpsc, prelude::*, stream::Fuse};
use libc::CAN_EFF_FLAG;
use nmea_parser::NmeaParser;
use orb_messages;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Debug},
    ops,
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Number of ring LEDs.
pub const RING_LED_COUNT: usize = 224;

/// Number of operator LEDs.
pub const OPERATOR_LED_COUNT: usize = 5;

/// Number of center LEDs.
pub const CENTER_LED_COUNT: usize = 9;

/// Operator LED sequence.
pub type RingLedsSequence = Vec<Rgb>;

/// Operator LED sequence.
pub type OperatorLedsSequence = [Rgb; OPERATOR_LED_COUNT];

/// Center LED sequence.
pub type CenterLedsSequence = [Rgb; CENTER_LED_COUNT];

const INPUT_CAPACITY: usize = 100;
const OUTPUT_CAPACITY: usize = 100;

/// Custom RGB color.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct RgbLed {
    red: u8,
    green: u8,
    blue: u8,
}

impl RgbLed {
    /// Constructs a new `RgbLed` with `red`, `green`, and `blue` values.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

/// Main microcontroller interface.
pub struct Main;

/// Main microcontroller interface for the Orb hardware.
#[derive(Debug)]
pub struct Jetson {
    log: Option<Log>,
    input_tx: mpsc::Sender<(Input, Option<ResultSender>)>,
    serial_input_tx: mpsc::Sender<Input>,
    output_tx: broadcast::Sender<Output>,
    output_rx: Fuse<BroadcastStream<Output>>,
}

/// Main microcontroller interface which does nothing.
pub struct Fake {
    log: Option<Log>,
    input_tx: mpsc::Sender<(Input, Option<ResultSender>)>,
    output_tx: broadcast::Sender<Output>,
    output_rx: Fuse<BroadcastStream<Output>>,
}

/// Configuration history.
#[derive(Debug)]
pub struct Log {
    /// Triggering the IR Eye Camera parameter history.
    pub triggering_ir_eye_camera: TimeSeries<bool>,
    /// Triggering the IR Face Camera parameter history.
    pub triggering_ir_face_camera: TimeSeries<bool>,
    /// IR LED duration parameter history.
    pub ir_led_duration: TimeSeries<u16>,
    /// 740nm IR LED duration parameter history.
    pub ir_led_duration_740nm: TimeSeries<u16>,
    /// User LED brightness parameter history.
    pub user_led_brightness: TimeSeries<u8>,
    /// User LED brightness parameter history.
    pub user_led_pattern: TimeSeries<UserLedControl>,
    /// Liquid lens parameter history.
    pub liquid_lens: TimeSeries<Option<i16>>,
    /// Camera frame rate parameter history.
    pub frame_rate: TimeSeries<u16>,
    /// IR LED mode parameter history.
    pub ir_led: TimeSeries<IrLed>,
    /// Mirror angle parameter history.
    pub mirror: TimeSeries<(u32, i32)>,
    /// Mirror relative angle parameter history.
    pub mirror_relative: TimeSeries<(i32, i32)>,
    /// Center LEDs parameter history.
    pub center_leds: TimeSeries<CenterLedsSequence>,
    /// Operator LEDs parameter history.
    pub operator_leds: TimeSeries<OperatorLedsSequence>,
    ///Fan speed parameter history.
    pub fan_speed: TimeSeries<f32>,
    /// Fan speed parameter history.
    pub mirror_homing: TimeSeries<(MirrorHomingMode, MirrorHomingAngle)>,
    /// Voltage monitoring parameter history.
    pub voltage_monitoring_period: TimeSeries<u32>,
}

/// Message to be sent to the Main microcontroller.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "mcu_input", content = "value")]
pub enum Input {
    /// Start or stop triggering the IR Eye Camera.
    TriggeringIrEyeCamera(bool),
    /// Start or stop triggering the IR Front Camera.
    TriggeringIrFaceCamera(bool),
    /// Control on duration of IR LEDs after trigger.
    IrLedDuration(u16),
    /// Same as IrLedDuration, but for 740nm (larger maximum duty cycle).
    IrLedDuration740nm(u16),
    /// Control brightness of front-mounted User LEDs.
    UserLedBrightness(u8),
    /// Set pattern of front-mounted User LEDs.
    UserLedPattern(UserLedControl),
    /// Set timing budget for ToF.
    TofTiming(TofTiming),
    /// Start ToF sensor calibration.
    TofCalibration(u16),
    /// Shutdown the uC after the delay.
    Shutdown(u8),
    /// Set liquid lens current.
    LiquidLens(Option<i16>),
    /// Frame rate in frames per second.
    FrameRate(u16),
    /// Selects the currently active infrared LEDs.
    IrLed(IrLed),
    /// Request firmware version.
    Version,
    /// Sends the internal jetson temperature for the fan control.
    Temperature(u16),
    /// Set mirror angle.
    Mirror(u32, i32),
    /// Set mirror angle relative to its current position.
    MirrorRelative(i32, i32),
    /// Perform Mirror Autohoming.
    PerformMirrorHoming(MirrorHomingMode, MirrorHomingAngle),
    /// Set Fan Speed.
    FanSpeed(f32),
    /// Ring LED sequence.
    RingLeds(RingLedsSequence),
    /// Center LED sequence.
    CenterLeds(CenterLedsSequence),
    /// Operator LED sequence.
    OperatorLeds(OperatorLedsSequence),
    /// Request Voltage once.
    VoltageRequest,
    /// Request Voltage sending with a certain period in ms.
    /// Specifying a period equal to 0 means to send the voltages once and cancel any previous set period.
    VoltageRequestPeriod(u32),
    /// Request Value.
    ValueGet(Property),
    // FIXME: next inputs are obsolete and used only by orb-control-api for
    // testing purposes.
    /// Control brightness of front-mounted User LEDs.
    OperatorLedBrightness(u8),
    /// Set pattern of front-mounted User LEDs.
    OperatorLedPattern(OperatorLedControl),
    /// Set the focus values (target current in mA) for the liquid lens to be
    /// used during a focus sweep operation.
    IrEyeCameraFocusSweepValuesPolynomial(FocusSweepPolynomial),
    /// Perform a focus sweep using the IR eye camera.
    PerformIrEyeCameraFocusSweep,
    /// Set the angle values for the mirror to be used during a mirror sweep
    /// operation.
    IrEyeCameraMirrorSweepValuesPolynomial(MirrorSweepPolynomial),
    /// Perform a mirror sweep.
    PerformIrEyeCameraMirrorSweep,
}

/// Message received from the Main microcontroller.
#[derive(Clone, Debug)]
pub enum Output {
    /// Power button state change. `true` if pressed.
    Button(bool),
    /// Parsed GPS message.
    Gps(nmea_parser::ParsedMessage),
    /// Successful acknowledge for certain Input.
    SuccessAck(Input),
    /// Temperature sensors.
    Temperature(orb_messages::mcu_main::Temperature),
    /// Voltage sensors.
    Voltage(orb_messages::mcu_main::Voltage),
    /// Battery capacity.
    BatteryCapacity(orb_messages::mcu_main::BatteryCapacity),
    /// Battery voltage.
    BatteryVoltage(orb_messages::mcu_main::BatteryVoltage),
    /// Battery is charging.
    BatteryIsCharging(orb_messages::mcu_main::BatteryIsCharging),
    /// Battery general information.
    BatteryInfo(orb_messages::mcu_main::BatteryInfoHwFw),
    /// Battery reset reason.
    BatteryReset(orb_messages::mcu_main::BatteryResetReason),
    /// Battery diagnostics.
    BatteryDiagCommon(orb_messages::mcu_main::BatteryDiagnosticCommon),
    /// Battery diagnostics.
    BatteryDiagSafety(orb_messages::mcu_main::BatteryDiagnosticSafety),
    /// Battery diagnostics.
    BatteryDiagPermanentFail(orb_messages::mcu_main::BatteryDiagnosticPermanentFail),
    /// Battery diagnostics.
    BatteryInfoSocAndStatistics(orb_messages::mcu_main::BatteryInfoSocAndStatistics),
    /// Battery diagnostics.
    BatteryInfoMaxValues(orb_messages::mcu_main::BatteryInfoMaxValues),
    /// Mirror range.
    MotorRange(orb_messages::mcu_main::MotorRange),
    /// Fan status.
    FanStatus(orb_messages::mcu_main::FanStatus),
    /// Ambient Light
    AmbientLight(orb_messages::mcu_main::AmbientLight),
    /// MCU fatal error reason
    FatalError(orb_messages::mcu_main::FatalError),
    /// MCU Logs.
    Logs(String),
    /// Firmware versions in primary and secondary slots.
    Versions(super::main::Versions),
    /// 1D ToF distance in mm.
    TofDistance(u32),
    /// State of hardware component
    HardwareDiag(orb_messages::mcu_main::HardwareDiagnostic),
}

/// This message provides coefficients for evaluating the formula:
/// `focus(n) = a + bn + cn^2 + dn^3 + en^4 + fn^5`
/// where `n` = frame number from 0 ... `number_of_frames`
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[allow(missing_docs)]
pub struct FocusSweepPolynomial {
    pub coef_a: f32,
    pub coef_b: f32,
    pub coef_c: f32,
    pub coef_d: f32,
    pub coef_e: f32,
    pub coef_f: f32,
    pub number_of_frames: u32,
}

/// This message provides coefficients for evaluating these formulae:
/// `radius(n) = a + b*n + c*n^2`
/// `angle(n) = a + b*n + c*n^2`
/// where `n` = frame number from 0 ... `number_of_frames`
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[allow(missing_docs)]
pub struct MirrorSweepPolynomial {
    pub radius_coef_a: f32,
    pub radius_coef_b: f32,
    pub radius_coef_c: f32,
    pub angle_coef_a: f32,
    pub angle_coef_b: f32,
    pub angle_coef_c: f32,
    pub number_of_frames: u32,
}

/// Timing budget for ToF.
#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub enum TofTiming {
    /// 15 ms.
    T15,
    /// 20 ms.
    T20,
    /// 33 ms.
    T33,
    /// 50 ms.
    T50,
    /// 100 ms.
    T100,
    /// 200 ms.
    T200,
    /// 500 ms.
    T500,
}

/// Infrared LEDs.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
pub enum IrLed {
    /// 850 nm.
    #[serde(rename = "850")]
    L850,
    /// 940 nm.
    #[serde(rename = "940")]
    L940,
    /// 740 nm.
    #[serde(rename = "740")]
    L740,
    /// 850 nm left.
    #[serde(rename = "850_left")]
    L850Left,
    /// 850 nm right.
    #[serde(rename = "850_right")]
    L850Right,
    /// 940 nm left.
    #[serde(rename = "940_left")]
    L940Left,
    /// 940 nm right.
    #[serde(rename = "940_right")]
    L940Right,
    /// 850 nm continuously active with reduced current.
    #[serde(rename = "850_continuous")]
    L850Cont,
    /// Burst mode: 740nm -> 850nm -> 940nm sequence.
    #[serde(rename = "burst_mode")]
    Burst,
    /// None
    #[serde(rename = "None")]
    None,
}

/// RGB LED color.
#[derive(Eq, PartialEq, Copy, Clone, Default, Debug, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl ops::Mul<f64> for Rgb {
    type Output = Self;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn mul(self, rhs: f64) -> Self::Output {
        Rgb(
            (f64::from(self.0) * rhs) as u8,
            (f64::from(self.1) * rhs) as u8,
            (f64::from(self.2) * rhs) as u8,
        )
    }
}

impl ops::MulAssign<f64> for Rgb {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn mul_assign(&mut self, rhs: f64) {
        self.0 = (f64::from(self.0) * rhs) as u8;
        self.1 = (f64::from(self.1) * rhs) as u8;
        self.2 = (f64::from(self.2) * rhs) as u8;
    }
}

impl ops::Add for Rgb {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Rgb(self.0 + rhs.0, self.1 + rhs.1, self.2 + rhs.2)
    }
}

/// User Led Patterns.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum UserLedPattern {
    /// Off.
    Off,
    /// Rainbow Effect.
    RandomRainbow,
    /// All White.
    AllWhite,
    /// All White without center LEDs.
    AllWhiteNoCenter,
    /// All White, but center LEDs only.
    AllWhiteOnlyCenter,
    /// All Red.
    AllRed,
    /// All Green.
    AllGreen,
    /// All Blue.
    AllBlue,
    /// Pulsing White
    PulsingWhite,
    /// Custom RGB color
    CustomRgb(Rgb),
    /// Custom RGB color
    PulsingCustomRgb(Rgb, f32, u32),
}

/// User LED control
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct UserLedControl {
    /// Pattern
    pub pattern: UserLedPattern,
    /// ring = trigonometric circle, values in degrees
    pub start_angle: Option<u16>,
    /// +/-360º, positive: clockwise, negative: anticlockwise. None defaults to the full ring (360º)
    pub angle_length: Option<f64>,
}

/// Operator Led Patterns.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum OperatorLedPattern {
    /// Off.
    Off,
    /// All White.
    AllWhite,
    /// All Red.
    AllRed,
    /// All Green.
    AllGreen,
    /// All Blue.
    AllBlue,
    /// Custom RGB
    CustomRgb(RgbLed),
}

/// Operator LED control
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct OperatorLedControl {
    /// Pattern
    pub pattern: OperatorLedPattern,
    /// Mask to control the 5 LEDs. Max is ((1 << 5) - 1) = 0x1F.
    /// Least significant bit correspond to the right side when facing the Orb, left side
    /// from the operator behind the Orb
    pub mask: u32,
}

/// Modes for mirror autohoming.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MirrorHomingMode {
    /// Default with stall detection.
    StallDetection,
    /// Perform maximum range steps toward one direction and range/2 back to the center.
    OneBlockingEnd,
}

/// Angle for mirror autohoming.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MirrorHomingAngle {
    /// Perform autohoming on both angles at the same time.
    Both,
    /// Perform autohoming on the vertical angle only.
    Vertical,
    /// Perform autohoming on the horizontal angle only.
    Horizontal,
}

/// Property to get from the main microcontroller firmware
#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Property {
    /// Ask for firmware versions
    FirmwareVersions,
}

/// Mcu app version
#[derive(Clone, Debug, Default, Copy, Serialize, Deserialize)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    commit_hash: u32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}/{}", self.major, self.minor, self.patch, self.commit_hash)
    }
}

impl From<&orb_messages::mcu_main::FirmwareVersion> for Version {
    fn from(version: &orb_messages::mcu_main::FirmwareVersion) -> Self {
        let orb_messages::mcu_main::FirmwareVersion { major, minor, patch, commit_hash } = *version;
        Self { major, minor, patch, commit_hash }
    }
}

/// Firmware versions in primary and secondary slots
#[derive(Default, Copy, Clone, Serialize, Debug)]
pub struct Versions {
    /// Primary slot
    pub primary: Version,
    /// Secondary slot
    pub secondary: Version,
}

impl From<&orb_messages::mcu_main::Versions> for Versions {
    fn from(app_versions: &orb_messages::mcu_main::Versions) -> Self {
        Self {
            primary: app_versions.primary_app.as_ref().map(Into::into).unwrap_or_default(),
            secondary: app_versions.secondary_app.as_ref().map(Into::into).unwrap_or_default(),
        }
    }
}

impl Interface for Main {
    type Input = Input;
    type Log = Log;
    type Output = Output;

    const CAN_ADDRESS: u32 = 0x01 | CAN_EFF_FLAG;
    const PROTOCOL_VERSION: i32 = orb_messages::mcu_main::Version::Version0 as i32;

    fn log_input(log: &mut Log, input: &Input) {
        match *input {
            Input::IrLedDuration(ir_led_duration) => {
                log.ir_led_duration.push(ir_led_duration);
            }
            Input::IrLedDuration740nm(ir_led_duration_740nm) => {
                log.ir_led_duration_740nm.push(ir_led_duration_740nm);
            }
            Input::UserLedBrightness(user_led_brightness) => {
                log.user_led_brightness.push(user_led_brightness);
            }
            Input::UserLedPattern(user_led_control) => {
                log.user_led_pattern.push(user_led_control);
            }
            Input::LiquidLens(current) => {
                log.liquid_lens.push(current);
            }
            Input::FrameRate(frame_rate) => {
                log.frame_rate.push(frame_rate);
            }
            Input::IrLed(ir_led) => {
                log.ir_led.push(ir_led);
            }
            Input::Mirror(x, y) => {
                log.mirror.push((x, y));
            }
            Input::MirrorRelative(x, y) => {
                log.mirror_relative.push((x, y));
            }
            Input::TriggeringIrEyeCamera(trigger) => {
                log.triggering_ir_eye_camera.push(trigger);
            }
            Input::TriggeringIrFaceCamera(trigger) => {
                log.triggering_ir_face_camera.push(trigger);
            }
            Input::FanSpeed(percentage) => {
                log.fan_speed.push(percentage);
            }
            Input::CenterLeds(sequence) => {
                log.center_leds.push(sequence);
            }
            Input::OperatorLeds(sequence) => {
                log.operator_leds.push(sequence);
            }
            Input::VoltageRequestPeriod(period) => {
                log.voltage_monitoring_period.push(period);
            }
            Input::VoltageRequest => {
                log.voltage_monitoring_period.push(0);
            }
            Input::PerformMirrorHoming(..)
            | Input::ValueGet(..)
            | Input::TofTiming(..)
            | Input::TofCalibration(..)
            | Input::Shutdown(..)
            | Input::Version
            | Input::Temperature(..)
            | Input::OperatorLedBrightness(_)
            | Input::OperatorLedPattern(_)
            | Input::RingLeds(_)
            | Input::IrEyeCameraFocusSweepValuesPolynomial(_)
            | Input::PerformIrEyeCameraFocusSweep
            | Input::IrEyeCameraMirrorSweepValuesPolynomial(_)
            | Input::PerformIrEyeCameraMirrorSweep => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn input_to_message(
        input: &Input,
        ack_number: u32,
    ) -> Option<orb_messages::mcu_main::mcu_message::Message> {
        let payload = match input {
            Input::IrLed(ir_led) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::InfraredLeds(orb_messages::mcu_main::InfraredLeDs {
                    wavelength: match ir_led {
                        IrLed::L850 => orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength850nm as i32,
                        IrLed::L940 => orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength940nm as i32,
                        IrLed::L740 => orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength740nm as i32,
                        IrLed::L850Left => {
                            orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength850nmLeft as i32
                        }
                        IrLed::L850Right => {
                            orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength850nmRight as i32
                        }
                        IrLed::L940Left => {
                            orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength940nmLeft as i32
                        }
                        IrLed::L940Right => {
                            orb_messages::mcu_main::infrared_le_ds::Wavelength::Wavelength940nmRight as i32
                        }
                        IrLed::None => orb_messages::mcu_main::infrared_le_ds::Wavelength::None as i32,
                        IrLed::L850Cont | IrLed::Burst => return None,
                    },
                })
            }
            Input::IrLedDuration(on_duration) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::LedOnTime(orb_messages::mcu_main::LedOnTimeUs {
                    on_duration_us: u32::from(*on_duration),
                })
            }
            Input::IrLedDuration740nm(on_duration) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::LedOnTime740nm(orb_messages::mcu_main::LedOnTimeUs {
                    on_duration_us: u32::from(*on_duration),
                })
            }
            Input::UserLedBrightness(brightness) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::UserLedsBrightness(orb_messages::mcu_main::UserLeDsBrightness {
                    brightness: u32::from(*brightness),
                })
            }
            Input::UserLedPattern(pattern) => {
                let mut custom_rgb: Option<Rgb> = None;
                let mut pulsing_scale: Option<f32> = None;
                let mut pulsing_period_ms: Option<u32> = None;
                let pattern_value = match pattern.pattern {
                    UserLedPattern::Off => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::Off as i32
                    }
                    UserLedPattern::RandomRainbow => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::RandomRainbow as i32
                    }
                    UserLedPattern::AllWhite => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllWhite as i32
                    }
                    UserLedPattern::AllWhiteNoCenter => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllWhiteNoCenter as i32
                    }
                    UserLedPattern::AllWhiteOnlyCenter => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllWhiteOnlyCenter as i32
                    }
                    UserLedPattern::AllRed => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllRed as i32
                    }
                    UserLedPattern::AllGreen => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllGreen as i32
                    }
                    UserLedPattern::AllBlue => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::AllBlue as i32
                    }
                    UserLedPattern::PulsingWhite => {
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::PulsingWhite as i32
                    }
                    UserLedPattern::CustomRgb(rgb) => {
                        custom_rgb = Some(rgb);
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::Rgb as i32
                    }
                    UserLedPattern::PulsingCustomRgb(rgb, scale, period) => {
                        custom_rgb = Some(rgb);
                        pulsing_scale = Some(scale);
                        pulsing_period_ms = Some(period);
                        orb_messages::mcu_main::user_le_ds_pattern::UserRgbLedPattern::PulsingRgb as i32
                    }
                };
                orb_messages::mcu_main::jetson_to_mcu::Payload::UserLedsPattern(orb_messages::mcu_main::UserLeDsPattern {
                    pattern: pattern_value,
                    custom_color: custom_rgb.map(|Rgb(r, g, b)| orb_messages::mcu_main::RgbColor {
                        red: u32::from(r),
                        green: u32::from(g),
                        blue: u32::from(b),
                    }),
                    start_angle: match pattern.start_angle {
                        Some(start) => u32::from(start),
                        _ => 0,
                    },
                    angle_length: match pattern.angle_length {
                        #[allow(clippy::cast_possible_truncation)]
                        Some(length) => (length / 100. * 360.) as i32,
                        _ => 360_i32, // default is full ring if None
                    },
                    pulsing_scale: pulsing_scale.unwrap_or(DEFAULT_USER_LED_PULSING_SCALE),
                    pulsing_period_ms: pulsing_period_ms.unwrap_or(DEFAULT_USER_LED_PULSING_PERIOD),
                })
            }
            Input::Shutdown(delay) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::Shutdown(orb_messages::mcu_main::ShutdownWithDelay {
                    delay_s: u32::from(*delay),
                })
            }
            Input::Temperature(_temperature) => {
                // orb_messages::mcu_main::jetson_to_mcu::Payload::Temperature(orb_messages::Temperature {
                //     source: orb_messages::mcu_main::temperature::TemperatureSource::Jetson as i32,
                //     temperature_c: i32::from(*temperature),
                // })
                return None;
            }
            Input::Mirror(phi_angle_millidegrees, theta_angle_millidegrees) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::MirrorAngle(orb_messages::mcu_main::MirrorAngle {
                    horizontal_angle: 0,
                    vertical_angle: 0,
                    phi_angle_millidegrees: *phi_angle_millidegrees,
                    // We fixed this conversion in KK
                    theta_angle_millidegrees: u32::try_from(*theta_angle_millidegrees).unwrap(),
                    angle_type: orb_messages::mcu_main::MirrorAngleType::PhiTheta as i32,
                })
            }
            Input::MirrorRelative(phi_angle_millidegrees, theta_angle_millidegrees) => orb_messages::mcu_main::jetson_to_mcu::Payload::MirrorAngleRelative(
                orb_messages::mcu_main::MirrorAngleRelative {
                    horizontal_angle: 0,
                    vertical_angle: 0,
                    phi_angle_millidegrees: *phi_angle_millidegrees,
                    theta_angle_millidegrees: *theta_angle_millidegrees,
                    angle_type: orb_messages::mcu_main::MirrorAngleType::PhiTheta as i32
                },
            ),
            Input::TriggeringIrEyeCamera(triggering) => {
                if *triggering {
                    orb_messages::mcu_main::jetson_to_mcu::Payload::StartTriggeringIrEyeCamera(
                        orb_messages::mcu_main::StartTriggeringIrEyeCamera {},
                    )
                } else {
                    orb_messages::mcu_main::jetson_to_mcu::Payload::StopTriggeringIrEyeCamera(
                        orb_messages::mcu_main::StopTriggeringIrEyeCamera {},
                    )
                }
            }
            Input::TriggeringIrFaceCamera(triggering) => {
                if *triggering {
                    orb_messages::mcu_main::jetson_to_mcu::Payload::StartTriggeringIrFaceCamera(
                        orb_messages::mcu_main::StartTriggeringIrFaceCamera {},
                    )
                } else {
                    orb_messages::mcu_main::jetson_to_mcu::Payload::StopTriggeringIrFaceCamera(
                        orb_messages::mcu_main::StopTriggeringIrFaceCamera {},
                    )
                }
            }
            Input::PerformMirrorHoming(mode, angle) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::DoHoming(orb_messages::mcu_main::PerformMirrorHoming {
                    homing_mode: match mode {
                        MirrorHomingMode::StallDetection => {
                            orb_messages::mcu_main::perform_mirror_homing::Mode::StallDetection as i32
                        }
                        MirrorHomingMode::OneBlockingEnd => {
                            orb_messages::mcu_main::perform_mirror_homing::Mode::OneBlockingEnd as i32
                        }
                    },
                    angle: match angle {
                        MirrorHomingAngle::Both => {
                            orb_messages::mcu_main::perform_mirror_homing::Angle::Both as i32
                        }
                        MirrorHomingAngle::Vertical => {
                            orb_messages::mcu_main::perform_mirror_homing::Angle::VerticalTheta as i32
                        }
                        MirrorHomingAngle::Horizontal => {
                            orb_messages::mcu_main::perform_mirror_homing::Angle::HorizontalPhi as i32
                        }
                    },
                })
            }
            Input::LiquidLens(current) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::LiquidLens(orb_messages::mcu_main::LiquidLens {
                    current: i32::from(current.unwrap_or(0)),
                    enable: current.is_some(),
                })
            }
            Input::FanSpeed(percentage) =>
            {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                orb_messages::mcu_main::jetson_to_mcu::Payload::FanSpeed(orb_messages::mcu_main::FanSpeed {
                    payload: Some(orb_messages::mcu_main::fan_speed::Payload::Value(
                        (*percentage / 100.0 * f32::from(u16::MAX)) as u32,
                    )),
                })
            }
            Input::RingLeds(sequence) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::RingLedsSequence(orb_messages::mcu_main::UserRingLeDsSequence {
                    data_format: Some(
                        orb_messages::mcu_main::user_ring_le_ds_sequence::DataFormat::RgbUncompressed(
                            sequence.iter().flat_map(|&Rgb(r, g, b)| [r, g, b]).collect(),
                        ),
                    ),
                })
            }
            Input::CenterLeds(sequence) => orb_messages::mcu_main::jetson_to_mcu::Payload::CenterLedsSequence(
                orb_messages::mcu_main::UserCenterLeDsSequence {
                    data_format: Some(
                        orb_messages::mcu_main::user_center_le_ds_sequence::DataFormat::RgbUncompressed(
                            sequence.iter().flat_map(|&Rgb(r, g, b)| [r, g, b]).collect(),
                        ),
                    ),
                },
            ),
            Input::OperatorLeds(sequence) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::DistributorLedsSequence(
                    orb_messages::mcu_main::DistributorLeDsSequence {
                        data_format: Some(
                            orb_messages::mcu_main::distributor_le_ds_sequence::DataFormat::RgbUncompressed(
                                sequence.iter().flat_map(|&Rgb(r, g, b)| [r, g, b]).collect(),
                            ),
                        ),
                    },
                )
            }
            Input::FrameRate(fps) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::Fps(orb_messages::mcu_main::Fps { fps: u32::from(*fps) })
            }
            Input::ValueGet(Property::FirmwareVersions) | Input::Version => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::ValueGet(orb_messages::mcu_main::ValueGet {
                    value: orb_messages::mcu_main::value_get::Value::FirmwareVersions as i32,
                })
            }
            Input::OperatorLedBrightness(brightness) => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::DistributorLedsBrightness(
                    orb_messages::mcu_main::DistributorLeDsBrightness { brightness: u32::from(*brightness) },
                )
            }
            Input::OperatorLedPattern(pattern) => {
                let mut custom_rgb: Option<RgbLed> = None;
                let pattern_value = match pattern.pattern {
                    OperatorLedPattern::Off => {
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::Off as i32
                    }
                    OperatorLedPattern::AllWhite => {
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::AllWhite
                            as i32
                    }
                    OperatorLedPattern::AllRed => {
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::AllRed as i32
                    }
                    OperatorLedPattern::AllGreen => {
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::AllGreen
                            as i32
                    }
                    OperatorLedPattern::AllBlue => {
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::AllBlue
                            as i32
                    }
                    OperatorLedPattern::CustomRgb(rgb) => {
                        custom_rgb = Some(rgb);
                        orb_messages::mcu_main::distributor_le_ds_pattern::DistributorRgbLedPattern::Rgb as i32
                    }
                };
                orb_messages::mcu_main::jetson_to_mcu::Payload::DistributorLedsPattern(
                    orb_messages::mcu_main::DistributorLeDsPattern {
                        pattern: pattern_value,
                        custom_color: custom_rgb.map(|rgb| orb_messages::mcu_main::RgbColor {
                            red: u32::from(rgb.red),
                            green: u32::from(rgb.green),
                            blue: u32::from(rgb.blue),
                        }),
                        leds_mask: pattern.mask,
                    },
                )
            }
            Input::TofTiming(_)
            | Input::TofCalibration(_) => {
                return None;
            },
            Input::VoltageRequest =>
                orb_messages::mcu_main::jetson_to_mcu::Payload::VoltageRequest(orb_messages::mcu_main::VoltageRequest {
                    transmit_period_ms: 0_u32,
                }),
            Input::VoltageRequestPeriod(period) => {
                tracing::info!("Setting voltage request period to {} ms", period);
                orb_messages::mcu_main::jetson_to_mcu::Payload::VoltageRequest(orb_messages::mcu_main::VoltageRequest {
                    transmit_period_ms: *period,
                })
            }
            Input::IrEyeCameraFocusSweepValuesPolynomial(FocusSweepPolynomial {
                                                             coef_a,
                                                             coef_b,
                                                             coef_c,
                                                             coef_d,
                                                             coef_e,
                                                             coef_f,
                                                             number_of_frames,
                                                         }) => orb_messages::mcu_main::jetson_to_mcu::Payload::IrEyeCameraFocusSweepValuesPolynomial(
                orb_messages::mcu_main::IrEyeCameraFocusSweepValuesPolynomial {
                    coef_a: *coef_a,
                    coef_b: *coef_b,
                    coef_c: *coef_c,
                    coef_d: *coef_d,
                    coef_e: *coef_e,
                    coef_f: *coef_f,
                    number_of_frames: *number_of_frames,
                },
            ),
            Input::PerformIrEyeCameraFocusSweep => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::PerformIrEyeCameraFocusSweep(
                    orb_messages::mcu_main::PerformIrEyeCameraFocusSweep {},
                )
            }
            Input::IrEyeCameraMirrorSweepValuesPolynomial(MirrorSweepPolynomial {
                radius_coef_a,
                radius_coef_b,
                radius_coef_c,
                angle_coef_a,
                angle_coef_b,
                angle_coef_c,
                number_of_frames,
            }) => orb_messages::mcu_main::jetson_to_mcu::Payload::IrEyeCameraMirrorSweepValuesPolynomial(
                orb_messages::mcu_main::IrEyeCameraMirrorSweepValuesPolynomial {
                    radius_coef_a: *radius_coef_a,
                    radius_coef_b: *radius_coef_b,
                    radius_coef_c: *radius_coef_c,
                    angle_coef_a: *angle_coef_a,
                    angle_coef_b: *angle_coef_b,
                    angle_coef_c: *angle_coef_c,
                    number_of_frames: *number_of_frames,
                },
            ),
            Input::PerformIrEyeCameraMirrorSweep => {
                orb_messages::mcu_main::jetson_to_mcu::Payload::PerformIrEyeCameraMirrorSweep(
                    orb_messages::mcu_main::PerformIrEyeCameraMirrorSweep {},
                )
            }
        };
        Some(orb_messages::mcu_main::mcu_message::Message::JMessage(
            orb_messages::mcu_main::JetsonToMcu { ack_number, payload: Some(payload) },
        ))
    }

    fn output_from_message(
        message: orb_messages::mcu_main::mcu_to_jetson::Payload,
        nmea_parser: &mut NmeaParser,
        nmea_prev_part: &mut Option<(u32, String)>,
    ) -> Option<Output> {
        match message {
            orb_messages::mcu_main::mcu_to_jetson::Payload::PowerButton(
                orb_messages::mcu_main::PowerButton { pressed },
            ) => Some(Output::Button(pressed)),
            orb_messages::mcu_main::mcu_to_jetson::Payload::Gnss(
                orb_messages::mcu_main::GnssData { nmea },
            ) => match nmea_parser.parse_sentence(&nmea) {
                Ok(message) => Some(Output::Gps(message)),
                Err(err) => {
                    tracing::error!("Error parsing NMEA: {err:?}");
                    None
                }
            },
            orb_messages::mcu_main::mcu_to_jetson::Payload::GnssPartial(
                orb_messages::mcu_main::GnssDataPartial { counter, nmea_part },
            ) => {
                if counter % 2 == 0 {
                    *nmea_prev_part = Some((counter, nmea_part));
                } else if let Some((counter_prev, nmea_prev_part)) = nmea_prev_part.take() {
                    if counter == counter_prev.wrapping_add(1) {
                        let nmea = format!("{nmea_prev_part}{nmea_part}");
                        match nmea_parser.parse_sentence(&nmea) {
                            Ok(message) => return Some(Output::Gps(message)),
                            Err(err) => tracing::error!("Error parsing NMEA: {err:?}"),
                        }
                    }
                }
                None
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::Temperature(temperature) => {
                Some(Output::Temperature(temperature))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::Log(orb_messages::mcu_main::Log {
                log,
            }) => Some(Output::Logs(log)),
            orb_messages::mcu_main::mcu_to_jetson::Payload::Voltage(voltage) => {
                Some(Output::Voltage(voltage))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::MotorRange(motor_range) => {
                Some(Output::MotorRange(motor_range))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::Versions(versions) => {
                Some(Output::Versions(Versions::from(&versions)))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryCapacity(capacity) => {
                Some(Output::BatteryCapacity(capacity))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryVoltage(battery_voltage) => {
                Some(Output::BatteryVoltage(battery_voltage))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryIsCharging(is_charging) => {
                Some(Output::BatteryIsCharging(is_charging))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryInfoHwFw(battery_info) => {
                Some(Output::BatteryInfo(battery_info))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryResetReason(reason) => {
                Some(Output::BatteryReset(reason))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryDiagCommon(diag) => {
                Some(Output::BatteryDiagCommon(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryDiagSafety(diag) => {
                Some(Output::BatteryDiagSafety(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryDiagPermanentFail(diag) => {
                Some(Output::BatteryDiagPermanentFail(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryInfoMaxValues(diag) => {
                Some(Output::BatteryInfoMaxValues(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::BatteryInfoSocAndStatistics(diag) => {
                Some(Output::BatteryInfoSocAndStatistics(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::Tof1d(distance) => {
                Some(Output::TofDistance(distance.distance_mm))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::FanStatus(status) => {
                Some(Output::FanStatus(status))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::FrontAls(als) => {
                Some(Output::AmbientLight(als))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::FatalError(error) => {
                Some(Output::FatalError(error))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::HardwareDiag(diag) => {
                Some(Output::HardwareDiag(diag))
            }
            orb_messages::mcu_main::mcu_to_jetson::Payload::Ack(_)
            | orb_messages::mcu_main::mcu_to_jetson::Payload::ImuData(_)
            | orb_messages::mcu_main::mcu_to_jetson::Payload::MemfaultEvent(_)
            | orb_messages::mcu_main::mcu_to_jetson::Payload::ConePresent(_)
            | orb_messages::mcu_main::mcu_to_jetson::Payload::Hardware(_) => None,
        }
    }

    fn success_ack_output_from_input(input: Input) -> Output {
        Output::SuccessAck(input)
    }
}

impl Jetson {
    /// Spawns a new microcontroller interface.
    pub fn spawn() -> Result<Self> {
        let (input_tx, input_rx) = mpsc::channel(INPUT_CAPACITY);
        let (output_tx, output_rx) = broadcast::channel(OUTPUT_CAPACITY);
        let (serial_input_tx, serial_input_rx) = mpsc::channel(INPUT_CAPACITY);
        let output_rx = BroadcastStream::new(output_rx).fuse();
        Can::<Main>::spawn(input_rx, output_tx.clone())?;
        Serial::<Main>::spawn(serial_input_rx)?;
        Ok(Self { log: None, input_tx, serial_input_tx, output_tx, output_rx })
    }
}

impl Mcu<Main> for Jetson {
    fn clone(&self) -> Box<dyn Mcu<Main>> {
        Box::new(Self {
            log: None,
            input_tx: self.input_tx.clone(),
            serial_input_tx: self.serial_input_tx.clone(),
            output_tx: self.output_tx.clone(),
            output_rx: BroadcastStream::new(self.output_tx.subscribe()).fuse(),
        })
    }

    fn tx(&self) -> &mpsc::Sender<(Input, Option<ResultSender>)> {
        &self.input_tx
    }

    fn tx_mut(&mut self) -> &mut mpsc::Sender<(Input, Option<ResultSender>)> {
        &mut self.input_tx
    }

    fn rx(&self) -> &Fuse<BroadcastStream<Output>> {
        &self.output_rx
    }

    fn rx_mut(&mut self) -> &mut Fuse<BroadcastStream<Output>> {
        &mut self.output_rx
    }

    fn log_mut(&mut self) -> &mut Option<Log> {
        &mut self.log
    }

    fn send_uart(&mut self, input: Input) -> Result<()> {
        self.serial_input_tx.send_now(input)
    }
}

impl Default for Fake {
    fn default() -> Self {
        let (input_tx, _) = mpsc::channel(INPUT_CAPACITY);
        let (output_tx, output_rx) = broadcast::channel(OUTPUT_CAPACITY);
        let output_rx = BroadcastStream::new(output_rx).fuse();
        Self { log: None, input_tx, output_tx, output_rx }
    }
}

impl Mcu<Main> for Fake {
    fn clone(&self) -> Box<dyn Mcu<Main>> {
        Box::new(Self {
            log: None,
            input_tx: self.input_tx.clone(),
            output_tx: self.output_tx.clone(),
            output_rx: BroadcastStream::new(self.output_tx.subscribe()).fuse(),
        })
    }

    fn tx(&self) -> &mpsc::Sender<(Input, Option<ResultSender>)> {
        &self.input_tx
    }

    fn tx_mut(&mut self) -> &mut mpsc::Sender<(Input, Option<ResultSender>)> {
        &mut self.input_tx
    }

    fn rx(&self) -> &Fuse<BroadcastStream<Output>> {
        &self.output_rx
    }

    fn rx_mut(&mut self) -> &mut Fuse<BroadcastStream<Output>> {
        &mut self.output_rx
    }

    fn log_mut(&mut self) -> &mut Option<Log> {
        &mut self.log
    }

    fn send_uart(&mut self, _input: Input) -> Result<()> {
        Ok(())
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            triggering_ir_eye_camera: TimeSeries::builder().limit(1_000_000).build(),
            triggering_ir_face_camera: TimeSeries::builder().limit(1_000_000).build(),
            ir_led_duration: TimeSeries::builder().limit(1_000_000).build(),
            ir_led_duration_740nm: TimeSeries::builder().limit(1_000_000).build(),
            user_led_brightness: TimeSeries::builder().limit(1_000_000).build(),
            user_led_pattern: TimeSeries::builder().limit(1_000_000).build(),
            liquid_lens: TimeSeries::builder().limit(1_000_000).build(),
            frame_rate: TimeSeries::builder().limit(1_000_000).build(),
            ir_led: TimeSeries::builder().limit(1_000_000).build(),
            mirror: TimeSeries::builder().limit(1_000_000).build(),
            mirror_relative: TimeSeries::builder().limit(1_000_000).build(),
            fan_speed: TimeSeries::builder().limit(1_000_000).build(),
            center_leds: TimeSeries::builder().limit(1_000_000).build(),
            operator_leds: TimeSeries::builder().limit(1_000_000).build(),
            mirror_homing: TimeSeries::builder().limit(1_000_000).build(),
            voltage_monitoring_period: TimeSeries::builder().limit(1_000_000).build(),
        }
    }
}
