//! Overcapture extension.
//!
//! The purpose of this extension is to capture more
//! than one frame per eye after we properly targeted the eye.

use crate::{
    agents::{
        camera,
        python::{ir_net, rgb_net},
    },
    brokers::{Orb, OrbPlan},
    consts::IR_CAMERA_FRAME_RATE,
    mcu::{self, main::IrLed},
    plans::{biometric_capture, biometric_capture::Output},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::VecDeque,
    task::Context,
    time::{Duration, Instant},
};
use tokio::time::sleep;

use super::ExtensionReport;

/// Camera FPS used during overcapture phase.
pub const CAPTURE_FPS: u16 = 60;

/// Default duration of the overcapture extension \[ms\].
pub const DEFAULT_OVERCAPTURE_DURATION: u64 = 1000;

/// Lookup table for mapping 3-bit configuration value to IR LED
/// configurations to use for extension. Order maps to digit positions.
pub const WAVELENGTH_LUT: [IrLed; 3] = [IrLed::L740, IrLed::L940, IrLed::L850];

/// Repoort of the overcapture extension configuration.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Report {
    overcapture_configuration: Configuration,
}

/// Overcapture extension to biometric capture plan.
///
/// See [the module-level documentation](self) for details.
pub struct Plan {
    biometric_capture: biometric_capture::Plan,
    state: State,
    configuration: Configuration,
    report: Report,
}

enum State {
    NoSharpIris,
    Overcapturing { start_time: Instant },
}

/// Configuration of overcapture extension.
///
/// Defines which wavelengths and for how long the overcapture
/// should run. The wavelength encoding is the same as for the
/// other extensions (octal value).
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct Configuration {
    wavelength_parameter: u8,
    overcapture_wavelengths: VecDeque<IrLed>,
    overcapture_duration: Duration,
}

impl OrbPlan for Plan {
    fn handle_ir_net(
        &mut self,
        orb: &mut Orb,
        output: port::Output<ir_net::Model>,
        frame: Option<camera::ir::Frame>,
    ) -> Result<BrokerFlow> {
        self.biometric_capture.handle_ir_net(orb, output, frame)
    }

    fn handle_rgb_net(
        &mut self,
        orb: &mut Orb,
        output: port::Output<rgb_net::Model>,
        frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        self.biometric_capture.handle_rgb_net(orb, output, frame)
    }

    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        match self.state {
            State::NoSharpIris => self.biometric_capture.poll_extra(orb, cx),
            State::Overcapturing { start_time } => {
                if start_time.elapsed() > self.configuration.overcapture_duration {
                    tracing::info!("Overcapture extension: Finished capture");
                    self.state = State::NoSharpIris;
                    Ok(BrokerFlow::Break)
                } else {
                    Ok(BrokerFlow::Continue)
                }
            }
        }
    }
}

impl From<biometric_capture::Plan> for Plan {
    fn from(biometric_capture: biometric_capture::Plan) -> Self {
        let config = biometric_capture
            .signup_extension_config
            .as_ref()
            .and_then(|config| config.parameters.as_ref());
        let (wavelength_parameter, duration_parameter) =
            config.map_or((1, Duration::from_millis(DEFAULT_OVERCAPTURE_DURATION)), |parameters| {
                parameters.split_once(':').map_or(
                    (
                        parse_u8_octal(parameters, 1),
                        Duration::from_millis(DEFAULT_OVERCAPTURE_DURATION),
                    ),
                    |(wavelength, duration)| {
                        (
                            parse_u8_octal(wavelength, 1),
                            parse_duration(duration, DEFAULT_OVERCAPTURE_DURATION),
                        )
                    },
                )
            });
        let configuration = Configuration::new(wavelength_parameter, duration_parameter);
        let report = Report { overcapture_configuration: configuration.clone() };
        Self { biometric_capture, state: State::NoSharpIris, configuration, report }
    }
}

impl Plan {
    /// Runs the biometric capture plan with overcapture extension.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Output> {
        self.biometric_capture.run_pre(orb).await?;
        self.reset_extension();
        // NOTE: Disabling the distance agent to prevent signup UX sound loops from interfering
        // with the "start" and "end" sounds
        orb.disable_distance();
        loop {
            orb.run(&mut self).await?;
            sleep(Duration::from_millis(1000)).await;
            while !self.extension_finished(orb).await? {
                self.perform_overcapture(orb).await?;
            }
            if self.biometric_capture.run_check(orb).await? {
                break;
            }
        }
        orb.enable_distance()?;
        self.biometric_capture.run_post(orb, Some(ExtensionReport::Overcapture(self.report))).await
    }

    async fn perform_overcapture(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Overcapture extension: Beginning capture");
        self.state = State::Overcapturing { start_time: Instant::now() };

        orb.main_mcu.send(mcu::main::Input::FrameRate(CAPTURE_FPS)).await?;
        orb.disable_ir_net();
        orb.disable_ir_auto_focus();
        orb.disable_mirror();
        orb.disable_eye_tracker();
        orb.disable_eye_pid_controller();
        orb.ir_eye_save_fps_override = Some(f32::INFINITY);
        orb.ir_face_save_fps_override = Some(f32::INFINITY);
        orb.thermal_save_fps_override = Some(f32::INFINITY);

        orb.run(self).await?;

        orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
        orb.enable_ir_net().await?;
        orb.enable_ir_auto_focus()?;
        orb.enable_mirror()?;
        orb.enable_eye_tracker()?;
        orb.enable_eye_pid_controller()?;
        orb.ir_eye_save_fps_override = None;
        orb.ir_face_save_fps_override = None;
        orb.thermal_save_fps_override = None;

        Ok(())
    }

    /// Check if extension has finished, i.e. all configured wavelengths have been captured.
    async fn extension_finished(&mut self, orb: &mut Orb) -> Result<bool> {
        if let Some(wavelength) = self.configuration.overcapture_wavelengths.pop_front() {
            orb.set_ir_wavelength(wavelength).await?;
            Ok(false)
        } else {
            self.reset_extension();
            Ok(true)
        }
    }

    /// Reset overcapture extension to starting configuration, i.e. prepare running for second eye.
    pub fn reset_extension(&mut self) {
        self.configuration.reset();
    }
}

impl Configuration {
    fn new(wavelength_parameter: u8, duration: Duration) -> Self {
        let overcapture_wavelengths =
            Configuration::parse_wavelength_configuration(wavelength_parameter);
        Self { wavelength_parameter, overcapture_wavelengths, overcapture_duration: duration }
    }

    fn reset(&mut self) {
        self.overcapture_wavelengths =
            Configuration::parse_wavelength_configuration(self.wavelength_parameter);
    }

    fn parse_wavelength_configuration(configuration_value: u8) -> VecDeque<IrLed> {
        let binary_repr = format!("{configuration_value:0>3b}");
        assert_eq!(
            binary_repr.chars().count(),
            3,
            "Overcapture wavelength parameter needs to be octal (<=3 bit)!"
        );
        let mut wavelength_vec = VecDeque::new();
        for (n, c) in binary_repr.char_indices() {
            if c == '1' {
                wavelength_vec.push_front(WAVELENGTH_LUT[n]);
            }
        }
        wavelength_vec
    }
}

fn parse_u8_octal(src: &str, default: u8) -> u8 {
    u8::from_str_radix(src, 8).unwrap_or(default)
}

fn parse_duration(src: &str, default: u64) -> Duration {
    Duration::from_millis(src.parse::<u64>().unwrap_or(default))
}
