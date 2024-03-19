//! Health check.

pub mod ir_camera_fps;

use crate::brokers::Orb;
use eyre::Result;

/// Health check plan.
#[derive(Default)]
pub struct Plan {
    ir_camera_fps: ir_camera_fps::Plan,
}

impl Plan {
    /// Runs the health check plan.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<bool> {
        let mut success = true;
        success = success && self.ir_camera_fps.run(orb).await?;
        Ok(success)
    }
}
