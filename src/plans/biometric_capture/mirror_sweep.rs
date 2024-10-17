//! Mirror Sweep extension.
use super::ExtensionReport;
use crate::{
    agents::{
        camera, mirror,
        python::{ir_net, rgb_net},
    },
    brokers::{Orb, OrbPlan},
    consts::{
        IR_CAMERA_FRAME_RATE, MIRROR_PHI_MAX_DIAMOND, MIRROR_PHI_MAX_PEARL, MIRROR_PHI_MIN_DIAMOND,
        MIRROR_PHI_MIN_PEARL, MIRROR_THETA_MAX_DIAMOND, MIRROR_THETA_MAX_PEARL,
        MIRROR_THETA_MIN_DIAMOND, MIRROR_THETA_MIN_PEARL,
    },
    identification,
    mcu::{self, main::IrLed},
    plans::{biometric_capture, biometric_capture::Output},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::{future::Fuse, prelude::*};
use rand::Rng;
use schemars::JsonSchema;
use serde::Serialize;
use std::{
    collections::VecDeque,
    f32::consts::PI,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::time;

/// FPS rate at which images are being captured during extension.
pub const SWEEP_FPS: u16 = 30;

/// Number of frames to capture during the extension run.
pub const SWEEP_FRAMES: u32 = 100;

/// Number of rotations performed for the gimbal sweep.
pub const N_ROTATIONS: f32 = 3.0;

/// Radial change per full rotation of the gimbal sweep.
pub const DELTA_ROT: f32 = 2.0;

/// Lookup table for mapping 3-bit configuration value to IR LED
/// configurations to use for extension. Order maps to digit positions.
pub const WAVELENGTH_LUT: [IrLed; 3] = [IrLed::L740, IrLed::L940, IrLed::L850];

/// Pre-defined mirror sweep polynomial.
pub const MIRROR_SWEEP_POLYNOMIAL: mcu::main::MirrorSweepPolynomial =
    mcu::main::MirrorSweepPolynomial {
        radius_coef_a: 1.0,
        radius_coef_b: 0.03,
        radius_coef_c: 0.0003,
        angle_coef_a: 10.0,
        angle_coef_b: 0.188_495_56,
        angle_coef_c: 0.0,
        number_of_frames: SWEEP_FRAMES,
    };

/// Report of the mirror sweep extension configuration and metadata.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Report {
    sweep_metadata: Vec<SweepMetadata>,
    sweep_configuration: SweepConfiguration,
}

/// Metadata for each executed mirror sweep
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SweepMetadata {
    /// Start time of mirror sweep
    start_time: SystemTime,
    /// Stop time of mirror sweep
    end_time: SystemTime,
    /// Number of frames captured in sweep
    captured_frame_count: u32,
    /// IR wavelength active throughout sweep
    wavelength: IrLed,
    /// Sweep is of right user eye
    is_left_eye: bool,
    /// Mirror coordinates that spiral is centered around.
    spiral_center: (u32, u32),
    /// Calculated polynomial coefficients for sweep.
    sweep_polynomial: mcu::main::MirrorSweepPolynomial,
}

/// Mirror sweep extension to biometric capture plan.
///
/// See [the module-level documentation](self) for details.
pub struct Plan {
    biometric_capture: biometric_capture::Plan,
    frame_counter: u32,
    last_point: (u32, u32),
    timeout: Fuse<Pin<Box<time::Sleep>>>,
    configuration: SweepConfiguration,
    report: Report,
}

/// Configuration of mirror sweep to run.
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
    fn handle_mirror(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<mirror::Actuator>,
    ) -> Result<BrokerFlow> {
        self.last_point = output.value;
        Ok(BrokerFlow::Continue)
    }

    fn handle_ir_eye_camera(
        &mut self,
        _orb: &mut Orb,
        _output: port::Output<camera::ir::Sensor>,
    ) -> Result<BrokerFlow> {
        if self.frame_counter > 0 {
            tracing::debug!("Mirror sweep frame: {}", self.frame_counter);
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
            last_point: (0, 0),
            timeout: Fuse::terminated(),
            configuration,
            report,
        }
    }
}

impl Plan {
    /// Runs the biometric capture plan with mirror sweep extension.
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
                self.perform_mirror_sweep(orb).await?;
            }
            if self.biometric_capture.run_check(orb).await? {
                orb.enable_image_notary()?;
                break;
            }
        }
        self.biometric_capture.run_post(orb, Some(ExtensionReport::MirrorSweep(self.report))).await
    }

    async fn perform_mirror_sweep(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Mirror Sweep extension: Beginning sweep @{:?}", self.last_point);
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
        orb.ir_face_save_fps_override = Some(f32::INFINITY);
        orb.thermal_save_fps_override = Some(f32::INFINITY);
        let polynomial = make_polynomial();
        tracing::info!("Mirror Sweep polynomial: {polynomial:?}");
        orb.main_mcu
            .send(mcu::main::Input::IrEyeCameraMirrorSweepValuesPolynomial(polynomial.clone()))
            .await?;
        orb.main_mcu.send(mcu::main::Input::PerformIrEyeCameraMirrorSweep).await?;
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
            "Mirror Sweep extension: completed with {} dropped frames",
            self.frame_counter
        );
        let end_time = SystemTime::now();

        let metadata = SweepMetadata {
            start_time,
            end_time,
            captured_frame_count: SWEEP_FRAMES - self.frame_counter,
            wavelength: orb.ir_wavelength(),
            is_left_eye: !orb.target_left_eye(),
            spiral_center: self.last_point,
            sweep_polynomial: polynomial,
        };
        self.report.sweep_metadata.push(metadata);

        self.frame_counter = 0;
        self.timeout = Fuse::terminated();
        let (phi, theta) = self.last_point;
        let (phi, theta) = if identification::HARDWARE_VERSION.contains("Diamond") {
            (
                phi.clamp(MIRROR_PHI_MIN_DIAMOND, MIRROR_PHI_MAX_DIAMOND),
                theta.clamp(MIRROR_THETA_MIN_DIAMOND, MIRROR_THETA_MAX_DIAMOND),
            )
        } else {
            (
                phi.clamp(MIRROR_PHI_MIN_PEARL, MIRROR_PHI_MAX_PEARL),
                theta.clamp(MIRROR_THETA_MIN_PEARL, MIRROR_THETA_MAX_PEARL),
            )
        };
        orb.main_mcu.send_now(mcu::main::Input::Mirror(phi, theta))?;
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

    /// Reset mirror sweep to starting configuration, i.e. prepare running for second eye.
    pub fn reset_extension(&mut self) {
        self.configuration.reset();
    }

    /// Configure wavelengths to run during mirror sweep extension by parsing
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
            "Mirror Sweep configuration value needs to be octal (<=3 bit)!"
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
fn make_polynomial() -> mcu::main::MirrorSweepPolynomial {
    // Gimbal sweeps are defined as an archimedian spiral defining
    // the offset in radial and angular direction compared to the
    // last applied gimbal position.
    //
    // Calculating the delta to be applied instead of the absolute values
    // makes the math much easier by requiring us to only define the
    // archimedian spiral around the coordinate origin.
    //
    // The initial angle is determined as a random number to introduce an
    // additional source of variation in our collected data.
    mcu::main::MirrorSweepPolynomial {
        radius_coef_a: 10.0,
        radius_coef_b: DELTA_ROT * N_ROTATIONS / SWEEP_FRAMES as f32,
        radius_coef_c: 0.0,
        angle_coef_a: rand::thread_rng().gen::<f32>() * 2.0 * PI,
        angle_coef_b: 2.0 * PI * N_ROTATIONS / SWEEP_FRAMES as f32,
        angle_coef_c: 0.0,
        number_of_frames: SWEEP_FRAMES,
    }
}
