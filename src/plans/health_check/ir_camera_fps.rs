//! IR camera FPS check.

use crate::{
    agents::camera,
    brokers::{Orb, OrbPlan},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::prelude::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::time::{sleep, Duration, Sleep};

const TOTAL_TIME: Duration = Duration::from_secs(30);
const MIN_FPS: f32 = 15.0;

/// IR camera FPS check plan.
pub struct Plan {
    timeout: Pin<Box<Sleep>>,
    face_frames_counter: u32,
    eye_frames_counter: u32,
}

impl OrbPlan for Plan {
    fn handle_ir_face_camera(
        &mut self,
        _orb: &mut Orb,
        _output: port::Output<camera::ir::Sensor>,
    ) -> Result<BrokerFlow> {
        self.face_frames_counter += 1;
        Ok(BrokerFlow::Continue)
    }

    fn handle_ir_eye_camera(
        &mut self,
        _orb: &mut Orb,
        _output: port::Output<camera::ir::Sensor>,
    ) -> Result<BrokerFlow> {
        self.eye_frames_counter += 1;
        Ok(BrokerFlow::Continue)
    }

    fn poll_extra(&mut self, _orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        if let Poll::Ready(()) = self.timeout.poll_unpin(cx) {
            return Ok(BrokerFlow::Break);
        }
        Ok(BrokerFlow::Continue)
    }
}

impl Default for Plan {
    fn default() -> Self {
        Self { timeout: Box::pin(sleep(TOTAL_TIME)), face_frames_counter: 0, eye_frames_counter: 0 }
    }
}

impl Plan {
    /// Runs the IR camera FPS check plan.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<bool> {
        tracing::info!("IR camera FPS check: running");

        orb.start_ir_eye_camera().await?;
        orb.start_ir_face_camera().await?;
        orb.run(self).await?;
        orb.stop_ir_face_camera().await?;
        orb.stop_ir_eye_camera().await?;

        tracing::info!("----- Eye IR CAMERA Results -----");
        #[allow(clippy::cast_precision_loss)]
        let eye_fps = self.eye_frames_counter as f32 / TOTAL_TIME.as_secs_f32();
        tracing::info!("fps: {}", eye_fps);

        tracing::info!("----- Face IR CAMERA Results -----");
        #[allow(clippy::cast_precision_loss)]
        let face_fps = self.face_frames_counter as f32 / TOTAL_TIME.as_secs_f32();
        tracing::info!("fps: {}", face_fps);

        let success = eye_fps > MIN_FPS && face_fps > MIN_FPS;
        tracing::info!("IR camera FPS check: {}", if success { "OK!" } else { "FAILURE!" });
        Ok(success)
    }
}
