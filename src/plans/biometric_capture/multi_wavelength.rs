//! Multi-wavelength extension.
//!
//! The goal is to capture not just two identification images at 850nm (current
//! implementation), but one additional image per side per wavelength (940nm
//! and 740nm).

use crate::{
    agents::{
        camera,
        python::{ir_net, rgb_net},
    },
    brokers::{Orb, OrbPlan},
    mcu::main::IrLed,
    plans::{biometric_capture, biometric_capture::Output},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::prelude::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time;

/// Time for auto-exposure to converge.
const AUTO_EXPOSURE_WAIT_TIME: Duration = Duration::from_millis(400);

/// Multi-wavelength extension to biometric capture plan.
///
/// See [the module-level documentation](self) for details.
pub struct Plan {
    biometric_capture: biometric_capture::Plan,
    state: State,
    left_940nm: Option<camera::ir::Frame>,
    left_740nm: Option<camera::ir::Frame>,
    right_940nm: Option<camera::ir::Frame>,
    right_740nm: Option<camera::ir::Frame>,
}

enum State {
    Normal,
    ExtraWavelength { timer: Pin<Box<time::Sleep>>, target_left_eye: bool, target_740nm: bool },
}

impl From<biometric_capture::Plan> for Plan {
    fn from(biometric_capture: biometric_capture::Plan) -> Self {
        Self {
            biometric_capture,
            state: State::Normal,
            left_940nm: None,
            left_740nm: None,
            right_940nm: None,
            right_740nm: None,
        }
    }
}

impl OrbPlan for Plan {
    fn handle_ir_eye_camera(
        &mut self,
        orb: &mut Orb,
        output: port::Output<camera::ir::Sensor>,
    ) -> Result<BrokerFlow> {
        match &self.state {
            State::Normal => self.biometric_capture.handle_ir_eye_camera(orb, output),
            State::ExtraWavelength { target_left_eye, target_740nm, .. } => {
                let slot = match (target_left_eye, target_740nm) {
                    (true, true) => &mut self.left_740nm,
                    (true, false) => &mut self.left_940nm,
                    (false, true) => &mut self.right_740nm,
                    (false, false) => &mut self.right_940nm,
                };
                *slot = Some(output.value);
                Ok(BrokerFlow::Continue)
            }
        }
    }

    fn handle_ir_net(
        &mut self,
        orb: &mut Orb,
        output: port::Output<ir_net::Model>,
        frame: Option<camera::ir::Frame>,
    ) -> Result<BrokerFlow> {
        match &self.state {
            State::Normal => self.biometric_capture.handle_ir_net(orb, output, frame),
            State::ExtraWavelength { .. } => Ok(BrokerFlow::Continue),
        }
    }

    fn handle_rgb_net(
        &mut self,
        orb: &mut Orb,
        output: port::Output<rgb_net::Model>,
        frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        match &self.state {
            State::Normal => self.biometric_capture.handle_rgb_net(orb, output, frame),
            State::ExtraWavelength { .. } => Ok(BrokerFlow::Continue),
        }
    }

    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        match &mut self.state {
            State::Normal => self.biometric_capture.poll_extra(orb, cx),
            State::ExtraWavelength { timer, .. } => {
                if let Poll::Ready(()) = timer.poll_unpin(cx) {
                    Ok(BrokerFlow::Break)
                } else {
                    Ok(BrokerFlow::Continue)
                }
            }
        }
    }
}

impl Plan {
    /// Runs the biometric capture plan with multi-wavelength extension.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Output> {
        self.biometric_capture.run_pre(orb).await?;
        loop {
            orb.run(&mut self).await?;
            self.perform_multi_wavelength(orb).await?;
            if self.biometric_capture.run_check(orb).await? {
                break;
            }
        }
        let mut output = self.biometric_capture.run_post(orb, None).await?;
        if let Some(capture) = &mut output.capture {
            capture.eye_left.ir_frame_940nm = self.left_940nm;
            capture.eye_left.ir_frame_740nm = self.left_740nm;
            capture.eye_right.ir_frame_940nm = self.right_940nm;
            capture.eye_right.ir_frame_740nm = self.right_740nm;
        }
        Ok(output)
    }

    async fn perform_multi_wavelength(&mut self, orb: &mut Orb) -> Result<()> {
        orb.disable_ir_net();
        orb.disable_ir_auto_focus();
        orb.disable_eye_pid_controller();

        tracing::info!("Multi-wavelength extension: capturing 940nm");
        orb.set_ir_wavelength(IrLed::L940).await?;
        orb.set_ir_duration(1500)?;
        self.state = State::ExtraWavelength {
            timer: Box::pin(time::sleep(AUTO_EXPOSURE_WAIT_TIME)),
            target_left_eye: self.biometric_capture.target_left_eye,
            target_740nm: false,
        };
        orb.run(self).await?;

        tracing::info!("Multi-wavelength extension: capturing 740nm");
        orb.set_ir_wavelength(IrLed::L740).await?;
        orb.set_ir_duration(200)?;
        self.state = State::ExtraWavelength {
            timer: Box::pin(time::sleep(AUTO_EXPOSURE_WAIT_TIME)),
            target_left_eye: self.biometric_capture.target_left_eye,
            target_740nm: true,
        };
        orb.run(self).await?;

        orb.enable_ir_net().await?;
        orb.enable_ir_auto_focus()?;
        orb.enable_eye_pid_controller()?;
        self.state = State::Normal;
        Ok(())
    }
}
