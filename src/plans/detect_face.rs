//! Face detection.

use crate::{
    agents::{camera, python},
    brokers::{Orb, OrbPlan},
    consts::{RGB_FPS, RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH},
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::prelude::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::time;

/// Face detection plan.
pub struct Plan {
    timeout: Pin<Box<time::Sleep>>,
    face_detected: bool,
}

impl OrbPlan for Plan {
    fn handle_rgb_net(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<python::rgb_net::Model>,
        _frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        #[allow(clippy::match_wildcard_for_single_variants)]
        match output.value {
            python::rgb_net::Output::Estimate(estimate) => {
                self.face_detected = match estimate.primary() {
                    Some(prediction) => prediction.is_face_detected(),
                    None => false,
                };
                if self.face_detected { Ok(BrokerFlow::Break) } else { Ok(BrokerFlow::Continue) }
            }
            _ => Ok(BrokerFlow::Continue),
        }
    }

    fn poll_extra(&mut self, _orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        if let Poll::Ready(()) = self.timeout.poll_unpin(cx) {
            tracing::info!("Face detection timed out");
            return Ok(BrokerFlow::Break);
        }
        Ok(BrokerFlow::Continue)
    }
}

impl Plan {
    /// Creates a new face detection plan.
    #[must_use]
    pub fn new(timeout: time::Duration) -> Self {
        Self { timeout: Box::pin(time::sleep(timeout)), face_detected: false }
    }
}

impl Plan {
    /// Runs the face detection plan.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<bool> {
        orb.start_rgb_camera(RGB_FPS).await?;
        orb.enable_rgb_net(true).await?;
        orb.set_fisheye(RGB_REDUCED_WIDTH, RGB_REDUCED_HEIGHT, false).await?;
        orb.run(self).await?;
        orb.stop_rgb_camera().await?;
        orb.disable_rgb_net();
        Ok(self.face_detected)
    }
}
