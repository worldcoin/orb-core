//! Focus Sweep extension.
//!
//! The purpose of this extension is to capture images with variation
//! in sharpness to gather data for IR-Net sharpness training.

use super::ExtensionReport;
use crate::{
    agents::{
        camera, ir_auto_focus,
        python::{ir_net, rgb_net},
    },
    brokers::{Orb, OrbPlan},
    consts::IR_CAMERA_FRAME_RATE,
    mcu,
    mcu::main::IrLed,
    plans::{biometric_capture, biometric_capture::Output},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::{future::Fuse, prelude::*};
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::time;

/// FPS rate at which images are being captured during extension.
pub const SWEEP_FPS: u16 = 30;

/// Number of frames to capture during the extension run.
pub const SWEEP_FRAMES: u32 = 100;

/// Delta in focus values traversed from start of sweep until currently
/// focused position. The sweep traverses the range [f_c - sweep_delta,
/// f_c + sweep_delta] where f_c is the currently focused position.
pub const SWEEP_DELTA: u32 = 80;

/// Slope of sweep around center focus. Set this to 0 to create a
/// plateau around the sharpest eye focus. Values >0 will have the
/// sweep pass through the sharpest focus faster.
const SWEEP_SLOPE: f32 = 0.25;

/// Lookup table for mapping 3-bit configuration value to IR LED
/// configurations to use for extension. Order maps to digit positions.
pub const WAVELENGTH_LUT: [IrLed; 3] = [IrLed::L740, IrLed::L940, IrLed::L850];

/// Report of the focus sweep extension configuration and metadata.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Report {
    sweep_metadata: Vec<SweepMetadata>,
    sweep_configuration: SweepConfiguration,
}

/// Metadata for each executed focus sweep
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SweepMetadata {
    /// Start time of focus sweep
    start_time: SystemTime,
    /// Stop time of focus sweep
    end_time: SystemTime,
    /// Number of frames captured in sweep
    captured_frame_count: u32,
    /// IR wavelength active throughout sweep
    wavelength: IrLed,
    /// Sweep is of right user eye
    is_left_eye: bool,
    /// Focus setting that sweep is centered around
    center_focus: i16,
    /// Calculated polynomial coefficients for sweep.
    sweep_polynomial: mcu::main::FocusSweepPolynomial,
}

/// Focus sweep extension to biometric capture plan.
///
/// See [the module-level documentation](self) for details.
pub struct Plan {
    biometric_capture: biometric_capture::Plan,
    frame_counter: u32,
    last_focus: i16,
    timeout: Fuse<Pin<Box<time::Sleep>>>,
    configuration: SweepConfiguration,
    report: Report,
}

/// Configuration of focus sweep to run.
///
/// Defines which wavelengths are used during extension based on
/// `configuration_value`, which is an octal value whose binary
/// representation symbolizes the three different wavelengths
/// (LSB=850nm, MSB=740nm).
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct SweepConfiguration {
    configuration_value: u8,
    sweep_wavelengths: VecDeque<IrLed>,
}

impl OrbPlan for Plan {
    fn handle_ir_auto_focus(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<ir_auto_focus::Agent>,
    ) -> Result<BrokerFlow> {
        self.last_focus = output.value;
        Ok(BrokerFlow::Continue)
    }

    fn handle_ir_eye_camera(
        &mut self,
        _orb: &mut Orb,
        _output: port::Output<camera::ir::Sensor>,
    ) -> Result<BrokerFlow> {
        if self.frame_counter > 0 {
            tracing::debug!("Focus sweep frame: {}", self.frame_counter);
            self.frame_counter -= 1;
            if self.frame_counter == 0 {
                return Ok(BrokerFlow::Break);
            }
        }
        Ok(BrokerFlow::Continue)
    }

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
        if let Poll::Ready(()) = self.timeout.poll_unpin(cx) {
            return Ok(BrokerFlow::Break);
        }
        if self.frame_counter == 0 {
            self.biometric_capture.poll_extra(orb, cx)
        } else {
            Ok(BrokerFlow::Continue)
        }
    }
}

impl From<biometric_capture::Plan> for Plan {
    fn from(biometric_capture: biometric_capture::Plan) -> Self {
        // TODO: Handle parameter value `0` -> use default biometric capture wavelength(s) (currently causes timeout)
        // TODO: Move parsing of configuration flag to QR code scanning phase -> Handle/Fail early
        let configuration = SweepConfiguration::new(
            biometric_capture
                .signup_extension_config
                .as_ref()
                .and_then(|config| config.parameters.as_ref())
                .and_then(|parameter| u8::from_str_radix(parameter, 8).ok())
                .unwrap_or(1),
        );
        let report =
            Report { sweep_metadata: Vec::new(), sweep_configuration: configuration.clone() };
        Self {
            biometric_capture,
            frame_counter: 0,
            last_focus: 0,
            timeout: Fuse::terminated(),
            configuration,
            report,
        }
    }
}

impl Plan {
    /// Runs the biometric capture plan with focus sweep extension.
    ///
    /// # Panics
    ///
    /// If `wavelength` given to the [`biometric_capture::Plan::new`]
    /// constructor was empty.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Output> {
        self.biometric_capture.run_pre(orb).await?;
        self.reset_extension();
        orb.disable_image_notary();
        loop {
            orb.run(&mut self).await?;
            while !self.extension_finished(orb).await? {
                self.perform_focus_sweep(orb).await?;
            }
            if self.biometric_capture.run_check(orb).await? {
                orb.enable_image_notary()?;
                break;
            }
        }
        self.biometric_capture.run_post(orb, Some(ExtensionReport::FocusSweep(self.report))).await
    }

    async fn perform_focus_sweep(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Focus Sweep extension: Beginning sweep @{}", self.last_focus);
        let start_time = SystemTime::now();
        orb.ui.pause();
        orb.main_mcu.send(mcu::main::Input::TriggeringIrEyeCamera(false)).await?;
        orb.main_mcu.send(mcu::main::Input::FrameRate(SWEEP_FPS)).await?;
        orb.disable_ir_auto_focus();
        orb.disable_ir_net();
        orb.disable_rgb_net();
        orb.disable_mirror();
        orb.disable_distance();
        orb.disable_eye_tracker();
        orb.disable_eye_pid_controller();
        orb.enable_image_notary()?;
        orb.ir_eye_save_fps_override = Some(f32::INFINITY);
        let polynomial = make_polynomial(self.last_focus);
        tracing::info!("Focus Sweep polynomial: {polynomial:?}");
        orb.main_mcu
            .send(mcu::main::Input::IrEyeCameraFocusSweepValuesPolynomial(polynomial.clone()))
            .await?;
        orb.main_mcu.send(mcu::main::Input::PerformIrEyeCameraFocusSweep).await?;
        self.frame_counter = SWEEP_FRAMES;
        self.timeout = Box::pin(time::sleep(Duration::from_secs_f64(
            f64::from(SWEEP_FRAMES) / f64::from(SWEEP_FPS) * 1.1,
        )))
        .fuse();

        orb.run(self).await?;

        orb.ir_eye_save_fps_override = None;
        orb.enable_ir_auto_focus()?;
        orb.enable_ir_net().await?;
        orb.enable_rgb_net(true).await?;
        orb.enable_mirror()?;
        orb.enable_distance()?;
        orb.enable_eye_tracker()?;
        orb.enable_eye_pid_controller()?;
        orb.disable_image_notary();
        orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
        orb.main_mcu.send(mcu::main::Input::TriggeringIrEyeCamera(true)).await?;
        orb.ui.resume();
        tracing::info!(
            "Focus Sweep extension: completed with {} dropped frames",
            self.frame_counter
        );
        let end_time = SystemTime::now();

        let metadata = SweepMetadata {
            start_time,
            end_time,
            captured_frame_count: SWEEP_FRAMES - self.frame_counter,
            wavelength: orb.ir_wavelength(),
            is_left_eye: !orb.target_left_eye(),
            center_focus: self.last_focus,
            sweep_polynomial: polynomial,
        };
        self.report.sweep_metadata.push(metadata);

        self.frame_counter = 0;
        self.timeout = Fuse::terminated();
        Ok(())
    }

    /// Check if extension execution has finished, i.e. all configured wavelengths
    /// have run.
    async fn extension_finished(&mut self, orb: &mut Orb) -> Result<bool> {
        if let Some(wavelength) = self.configuration.sweep_wavelengths.pop_front() {
            orb.set_ir_wavelength(wavelength).await?;
            Ok(false)
        } else {
            self.reset_extension();
            Ok(true)
        }
    }

    /// Reset focus sweep to starting configuration, i.e. prepare running for second eye.
    pub fn reset_extension(&mut self) {
        self.configuration.reset();
    }

    /// Configure wavelengths to run during focus sweep extension by parsing
    /// `configuration_value`.
    #[must_use]
    pub fn configure(mut self, configuration_value: u8) -> Self {
        self.configuration = SweepConfiguration::new(configuration_value);
        self
    }
}

impl SweepConfiguration {
    fn new(configuration_value: u8) -> Self {
        Self {
            configuration_value,
            sweep_wavelengths: SweepConfiguration::parse_configuration_value(configuration_value),
        }
    }

    fn reset(&mut self) {
        self.sweep_wavelengths =
            SweepConfiguration::parse_configuration_value(self.configuration_value);
    }

    fn parse_configuration_value(configuration_value: u8) -> VecDeque<IrLed> {
        let binary_repr = format!("{configuration_value:0>3b}");
        assert_eq!(
            binary_repr.chars().count(),
            3,
            "Focus Sweep configuration value needs to be octal (<=3 bit)!"
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

#[allow(clippy::cast_precision_loss)]
fn make_polynomial(last_focus: i16) -> mcu::main::FocusSweepPolynomial {
    // Focus sweeps are performed as a polynomial of degree 3.
    // It is defined as a mapping from frame number (x) to focus value (y).
    //
    // The polynomial is normalized to pass through the following points:
    // x=0                : last_focus - SWEEP_DELTA
    // x=SWEEP_FRAMES / 2 : last_focus
    // x=SWEEP_FRAMES     : last_focus + SWEEP_DELTA
    // The polynomial will have derivative SWEEP_SLOPE at
    // x=SWEEP_FRAMES / 2.
    let f = f32::from(last_focus);
    let d_f = SWEEP_DELTA as f32;
    let s = SWEEP_SLOPE;
    let n = SWEEP_FRAMES as f32;
    mcu::main::FocusSweepPolynomial {
        coef_a: f - d_f,
        coef_b: -2.0 * s + 6.0 * d_f / n,
        coef_c: 6.0 * s / n - 12.0 * d_f / n.powi(2),
        coef_d: 8.0 * d_f / n.powi(3) - 4.0 * s / n.powi(2),
        coef_e: 0.0,
        coef_f: 0.0,
        number_of_frames: SWEEP_FRAMES,
    }
}
