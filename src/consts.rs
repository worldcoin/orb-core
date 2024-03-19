//! Project constants.

use crate::mcu::main::IrLed;
use sodiumoxide::crypto::box_;
use std::{ops::RangeInclusive, time::Duration};

/// Path to the configuration directory.
pub const CONFIG_DIR: &str = "/usr/persistent/";

/// Path to the configuration directory.
pub const RGB_CALIBRATION_FILE: &str = "rgb_calibration.json";

/// Path to the directory with the sound files.
pub const SOUNDS_DIR: &str = "/home/worldcoin/data/sounds";

/// ALSA sound card name.
pub const SOUND_CARD_NAME: &str = "default";

/// Default sound volume.
pub const DEFAULT_SOUND_VOLUME: u64 = 10;

/// Maximum sound volume.
pub const MAX_SOUND_VOLUME: u64 = 100;

/// Time to hold the button to shutdown.
pub const BUTTON_LONG_PRESS_DURATION: Duration = Duration::from_secs(2);

/// Maximum time window to do a triple press.
pub const BUTTON_TRIPLE_PRESS_DURATION: Duration = Duration::from_millis(700);

/// Maximum time window to do a double press.
pub const BUTTON_DOUBLE_PRESS_DURATION: Duration = Duration::from_millis(450);

/// Time window after a double press to ensure it's not a triple press.
pub const BUTTON_DOUBLE_PRESS_DEAD_TIME: Duration = Duration::from_millis(250);

/// Battery voltage threshold to shutdown the device when device in Idle state.
pub const BATTERY_VOLTAGE_SHUTDOWN_IDLE_THRESHOLD_MV: i32 = 13350;

/// Battery voltage threshold to shutdown the device when device in Idle state.
pub const BATTERY_VOLTAGE_SHUTDOWN_BIOMETRIC_CAPTURE_THRESHOLD_MV: i32 = 12500;

/// Delay left for Jetson to shut down. The microcontroller will force shut down after this delay
/// if no shutdown request signal is received.
pub const GRACEFUL_SHUTDOWN_MAX_DELAY_SECONDS: u8 = 20;

/// Backend config update interval.
pub const CONFIG_UPDATE_INTERVAL: Duration = Duration::from_secs(10);

/// Backend status update interval.
pub const STATUS_UPDATE_INTERVAL: Duration = Duration::from_secs(10);

/// QR code scanning timeout.
pub const QR_SCAN_TIMEOUT: Duration = Duration::from_secs(70);

/// QR code scanning reminder interval.
pub const QR_SCAN_REMINDER: Duration = Duration::from_secs(25);

/// Delay between operator QR code scanning & user QR code scanning.
pub const QR_SCAN_INTERVAL: Duration = Duration::from_millis(1500);

/// Face detection timeout.
pub const DETECT_FACE_TIMEOUT: Duration = Duration::from_secs(20);

/// Default IR (infrared) LED duration in microseconds.
pub const DEFAULT_IR_LED_DURATION: u16 = 350;

/// Minimum duration of IR (infrared) LEDs microseconds.
pub const IR_LED_MIN_DURATION: u16 = 10;

/// Maximum duration of IR (infrared) LEDs for wavelengths 850nm and 940nm in microseconds.
// FIXME `IR_LED_MAX_DURATION` and `IR_LED_MAX_DURATION_740NM` are
// pre-calculated for 30 FPS. When we make the FPS dynamic, these values should
// change with FPS.
pub const IR_LED_MAX_DURATION: u16 = 3333;

/// Maximum duration of IR (infrared) LEDs for 740nm wavelength in microseconds.
pub const IR_LED_MAX_DURATION_740NM: u16 = 15000;

/// IR (infrared) camera frame rate.
pub const IR_CAMERA_FRAME_RATE: u16 = 30;

/// Default gain for the IR (infrared) camera.
pub const IR_CAMERA_DEFAULT_GAIN: i64 = 0;

/// Initial exposure for the IR (infrared) camera in microseconds.
/// Set to the default IR (infrared) LED duration since the sensor should be exposed with same duration as the IR (infrared) LEDs.
pub const IR_CAMERA_DEFAULT_EXPOSURE: i64 = DEFAULT_IR_LED_DURATION as i64;

/// Default IR camera black level setting
pub const IR_CAMERA_DEFAULT_BLACK_LEVEL: i64 = 0;

/// Default IR (infrared) LED wavelength.
pub const DEFAULT_IR_LED_WAVELENGTH: IrLed = IrLed::L850;

/// Extra IR (infrared) LED wavelengths with duration.
pub const EXTRA_IR_LED_WAVELENGTHS: &[(IrLed, u16)] =
    /* &[(IrLed::L740, 200), (IrLed::L940, 1500)] */
    &[];

/// Width of IR camera.
pub const IR_WIDTH: u32 = 1440;

/// Height of IR camera.
pub const IR_HEIGHT: u32 = 1080;

/// Maximum width of RGB camera.
pub const RGB_NATIVE_WIDTH: u32 = 2464;

/// Maximum height of RGB camera.
pub const RGB_NATIVE_HEIGHT: u32 = 3280;

/// Converted width of RGB camera.
pub const RGB_DEFAULT_WIDTH: u32 = 1232;

/// Converted height of RGB camera.
pub const RGB_DEFAULT_HEIGHT: u32 = 1640;

// TODO: the RGB width and height should be increased as soon as the rgbnet bug for undistortion is fixed
/// Reduced width of RGB camera.
pub const RGB_REDUCED_WIDTH: u32 = 480;

/// Reduced height of RGB camera.
pub const RGB_REDUCED_HEIGHT: u32 = 640;

/// Calibration width of RGB camera.
pub const RGB_CALIBRATION_WIDTH: u32 = RGB_REDUCED_WIDTH;

/// Calibration height of RGB camera.
pub const RGB_CALIBRATION_HEIGHT: u32 = RGB_REDUCED_HEIGHT;

/// RGB camera exposure time range.
pub const RGB_EXPOSURE_RANGE: RangeInclusive<u32> = 34_000..=358_733_008;

/// RGB camera FPS.
pub const RGB_FPS: u32 = 20;

/// Number of columns in raw frames from the camera.
pub const THERMAL_WIDTH: u32 = 156;

/// Number of rows in raw frames from the camera.
pub const THERMAL_HEIGHT: u32 = 206;

/// LED engine FPS.
pub const LED_ENGINE_FPS: u64 = 60;

/// Default user LED brightness.
pub const USER_LED_DEFAULT_BRIGHTNESS: u8 = 2;

/// Default pulsing Scale for the pulsing LED pattern.
pub const DEFAULT_USER_LED_PULSING_SCALE: f32 = 2.0;

/// Default user led period for the pulsing LED pattern.
pub const DEFAULT_USER_LED_PULSING_PERIOD: u32 = 4000;

/// Focus lens min setting.
pub const AUTOFOCUS_MIN: i16 = -400;

/// Focus lens max setting.
pub const AUTOFOCUS_MAX: i16 = 400;

/// Minimum iris sharpness score to initiate scan.
pub const IRIS_SHARPNESS_MIN: f64 = 1.00; // TODO: put back 0.68

/// Minimum iris sharpness score for sign up.
pub const IRIS_SCORE_MIN: f64 = 1.70; // TODO: put back 0.68

/// Mean brightness range for sign up. Note: This is also handled by IRNet,
/// which doesn't currently provide a sharpness score for images unless they
/// have an in-range brightness.
pub const IRIS_BRIGHTNESS_RANGE: RangeInclusive<u8> = 80..=180;

/// Number of sharp IR (infrared) frames to save, for each wavelength
pub const NUM_SHARP_IR_FRAMES: usize = 10;

/// Tof distance in mm for best iris focus.
// TODO: calculated from Tobi (AI) measurements, find correct values
pub const IR_FOCUS_DISTANCE: f64 = 305.0;

/// Offset in mm for white LED. When in this range the focus capture time is
/// counted.
// TODO: calculated from Tobi (AI) measurements, find correct values
pub const IR_FOCUS_RANGE: RangeInclusive<f64> = 150.0..=460.0;

/// Initial focus range.
pub const IR_FOCUS_RANGE_SMALL: RangeInclusive<f64> = 190.0..=410.0;

/// FPS to save IR (infrared) eye images
pub const IR_EYE_SAVE_FPS: f32 = 0.5;

/// FPS to save IR (infrared) face images
pub const IR_FACE_SAVE_FPS: f32 = 0.5;

/// FPS to save RGB images
pub const RGB_SAVE_FPS: f32 = 0.5;

/// FPS to save Thermal images
pub const THERMAL_SAVE_FPS: f32 = 0.5;

/// Minimal interval between slower, closer or farther sounds.
pub const IR_VOICE_TIME_INTERVAL: Duration = Duration::from_secs(2);

/// Time to switch between eyes for one mirror.
pub const SWITCH_EYE_DELAY: Duration = Duration::from_millis(300);

/// Reducer coefficient for continuous calibration. Must be less than or equal
/// to `1.0`.
pub const CONTINUOUS_CALIBRATION_REDUCER: f64 = 0.05;

/// Timeout for the biometric capture phase.
pub const BIOMETRIC_CAPTURE_TIMEOUT: Duration = Duration::from_secs(45);

/// Maximum time for Wifi network connection state.
pub const NETWORK_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Minimum Fan Speed.
pub const MINIMUM_FAN_SPEED: f32 = 1.0;

/// Maximum Fan Speed.
/// Note: For EV1 (louder fan), this is scaled down via backend configuration
pub const MAXIMUM_FAN_SPEED: f32 = 80.0;

/// WC Data Encryption Pubkey
pub const WORLDCOIN_ENCRYPTION_PUBKEY: box_::PublicKey = {
    #[cfg(not(feature = "stage"))]
    {
        box_::PublicKey([
            0x1b, 0x38, 0x95, 0x88, 0x9f, 0x7d, 0xf8, 0x95, 0x05, 0x55, 0x66, 0x66, 0xde, 0xf7,
            0xb0, 0xc3, 0x1b, 0x35, 0xa4, 0x6e, 0x8a, 0xf1, 0x83, 0xe0, 0xf2, 0x3f, 0x2f, 0x1a,
            0x17, 0x73, 0x55, 0x05,
        ])
    }
    #[cfg(feature = "stage")]
    {
        box_::PublicKey([
            0xe1, 0xd1, 0x21, 0xbe, 0xc6, 0x07, 0x44, 0x89, 0xe8, 0x20, 0x38, 0x3b, 0xcc, 0x17,
            0xa2, 0x5f, 0xa5, 0xdb, 0xd9, 0x61, 0x24, 0x02, 0x24, 0x4d, 0xf7, 0x16, 0x42, 0x02,
            0xa5, 0x21, 0x13, 0x56,
        ])
    }
};

/// WC Data Encryption Private key (only on staging!)
// NOTE(open-source): This is a bogus key.
#[cfg(feature = "stage")]
pub const WORLDCOIN_ENCRYPTION_SECRETKEY: box_::SecretKey =
    box_::SecretKey([0xFF; box_::SECRETKEYBYTES]);

/// The well known name used as a bus name to register on dbus.
pub const DBUS_WELL_KNOWN_BUS_NAME: &str = "org.worldcoin.OrbCore1";

/// The name that the broker will use for the signup interface.
pub const DBUS_SIGNUP_INTERFACE_NAME: &str = "org.worldcoin.OrbCore1.Signup";

/// The object path under which the broker will advertise the signup interface.
pub const DBUS_SIGNUP_OBJECT_PATH: &str = "/org/worldcoin/OrbCore1/Signup";

// TODO: This should be a getter function from ir_net rather than a constant.
/// Threshold for a valid signup in terms of occlusion 30.
pub const THRESHOLD_OCCLUSION_30: f64 = 0.85;

/// Default maximum fan speed.
pub const DEFAULT_MAX_FAN_SPEED: f32 = 100.0;

/// Length of the across signup face correlation queue.
pub const ACROSS_SIGNUP_FACE_CORRELATION_QUEUE_LENGTH: usize = 100;

/// Default maximum ping delay in milliseconds for acceptable network connection.
pub const DEFAULT_SLOW_INTERNET_PING_THRESHOLD: Duration = Duration::from_millis(700);

/// By default block signups when no internet connection is available.
pub const DEFAULT_BLOCK_SIGNUPS_WHEN_NO_INTERNET: bool = true;

/// Default amount of time to wait until we assume the camera is stuck pairing.
pub const DEFAULT_THERMAL_CAMERA_PAIRING_STATUS_TIMEOUT: Duration = Duration::from_millis(2000);
