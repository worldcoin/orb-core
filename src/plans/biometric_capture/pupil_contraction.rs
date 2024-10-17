//! Pupil Contraction extension.
//!
//! The purpose of this extension is to force the pupil of a person to contract
//! during collection of images.

use crate::{
    agents::{
        camera,
        python::{ir_net, rgb_net},
    },
    brokers::{Orb, OrbPlan},
    consts::USER_LED_DEFAULT_BRIGHTNESS,
    mcu,
    plans::{biometric_capture, biometric_capture::Output},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use std::{
    task::Context,
    time::{Duration, Instant},
};

/// The time it takes going from LED brightness 0 to 255. Low times help keep
/// eye in focus and frame. High times help collecting more data with more
/// variants of pupil contraction.
pub const RAMP_TIME: Duration = Duration::from_millis(3000);

/// Physiologic pupil contraction takes some time to react to brightness. Will
/// have to look into how fast the contraction normally occurs.
pub const WAIT_TIME: Duration = Duration::from_millis(500);

/// FPS rate at which images are being captured during extension. Captured
/// images should cover the full range of pupil contraction variation. This
/// should be tuned to trade off amount of data to be uploaded and number of
/// images with varying pupil sizes.
pub const FPS: f32 = 6.0;

/// Pupil Contraction extension to biometric capture plan.
///
/// See [the module-level documentation](self) for details.
pub struct Plan {
    biometric_capture: biometric_capture::Plan,
    state: State,
}

enum State {
    NoSharpIris,
    RampingUp { start_time: Instant },
    Waiting { start_time: Instant },
}

impl OrbPlan for Plan {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn handle_ir_net(
        &mut self,
        orb: &mut Orb,
        output: port::Output<ir_net::Model>,
        frame: Option<camera::ir::Frame>,
    ) -> Result<BrokerFlow> {
        match self.state {
            State::NoSharpIris => {
                if let BrokerFlow::Break =
                    self.biometric_capture.handle_ir_net(orb, output, frame)?
                {
                    tracing::info!("Pupil Contraction extension: ramping up");
                    self.state = State::RampingUp { start_time: Instant::now() };
                    orb.main_mcu.send_now(mcu::main::Input::UserLedPattern(
                        mcu::main::UserLedControl {
                            pattern: mcu::main::UserLedPattern::AllWhite,
                            start_angle: Some(0),
                            angle_length: Some(100.),
                        },
                    ))?;
                    orb.main_mcu.send_now(mcu::main::Input::UserLedBrightness(0))?;
                    orb.disable_ir_auto_exposure();
                    orb.disable_ir_auto_focus();
                    orb.disable_eye_tracker();
                    orb.disable_eye_pid_controller();
                    orb.ir_eye_save_fps_override = Some(FPS);
                }
            }
            State::RampingUp { start_time } => {
                let now = Instant::now();
                let elapsed = now - start_time;
                let brightness = (elapsed.as_secs_f32() / RAMP_TIME.as_secs_f32()
                    * f32::from(u8::MAX))
                .clamp(0.0, u8::MAX.into()) as u8;
                orb.main_mcu.send_now(mcu::main::Input::UserLedPattern(
                    mcu::main::UserLedControl {
                        pattern: mcu::main::UserLedPattern::AllWhite,
                        start_angle: Some(0),
                        angle_length: Some(100.),
                    },
                ))?;
                orb.main_mcu.send_now(mcu::main::Input::UserLedBrightness(brightness))?;
                if elapsed >= RAMP_TIME {
                    tracing::info!("Pupil Contraction extension: waiting");
                    self.state = State::Waiting { start_time: now };
                }
            }
            State::Waiting { start_time } => {
                if start_time.elapsed() >= WAIT_TIME {
                    tracing::info!("Pupil Contraction extension: complete");
                    self.state = State::NoSharpIris;
                    orb.try_enable_ir_auto_exposure();
                    orb.try_enable_ir_auto_focus();
                    orb.try_enable_eye_tracker();
                    orb.try_enable_eye_pid_controller();
                    orb.ir_eye_save_fps_override = None;
                    orb.main_mcu.send_now(mcu::main::Input::UserLedBrightness(
                        USER_LED_DEFAULT_BRIGHTNESS,
                    ))?;
                    return Ok(BrokerFlow::Break);
                }
            }
        }
        Ok(BrokerFlow::Continue)
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
        self.biometric_capture.poll_extra(orb, cx)
    }
}

impl From<biometric_capture::Plan> for Plan {
    fn from(biometric_capture: biometric_capture::Plan) -> Self {
        Self { biometric_capture, state: State::NoSharpIris }
    }
}

impl Plan {
    /// Runs the biometric capture plan.
    ///
    /// # Panics
    ///
    /// If `wavelength` given to the [`biometric_capture::Plan::new`]
    /// constructor was empty.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Output> {
        self.biometric_capture.run_pre(orb).await?;
        loop {
            orb.run(&mut self).await?;
            if self.biometric_capture.run_check(orb).await? {
                break;
            }
        }
        self.biometric_capture.run_post(orb, None).await
    }
}
