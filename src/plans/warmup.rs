//! Warm up critical agents to make subsequent plans faster.

use crate::{
    agents::{
        camera,
        python::{face_identifier, ir_net, mega_agent_one, mega_agent_two, rgb_net},
    },
    brokers::{BrokerFlow, Orb, OrbPlan},
    monitor, port,
};
use eyre::{bail, Result};
use futures::prelude::*;
use std::time::Instant;

const CPU_OVERLOAD_THRESHOLD: f64 = 0.8;

/// Warm up plan.
#[derive(Default)]
pub struct Plan {
    ir_net_estimate_received: bool,
    rgb_net_estimate_received: bool,
    face_identifier_response_received: bool,
}

impl OrbPlan for Plan {
    fn handle_rgb_net(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<rgb_net::Model>,
        _frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        if let rgb_net::Output::Warmup = output.value {
            self.rgb_net_estimate_received = true;
            tracing::info!("RGB-Net warmed up");
        }
        Ok(if self.is_done() { BrokerFlow::Break } else { BrokerFlow::Continue })
    }

    fn handle_ir_net(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<ir_net::Model>,
        _frame: Option<camera::ir::Frame>,
    ) -> Result<BrokerFlow> {
        if let ir_net::Output::Warmup = output.value {
            self.ir_net_estimate_received = true;
            tracing::info!("IR-Net warmed up");
        }
        Ok(if self.is_done() { BrokerFlow::Break } else { BrokerFlow::Continue })
    }

    fn handle_face_identifier(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<face_identifier::Model>,
        _frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        if let face_identifier::Output::Warmup = output.value {
            self.face_identifier_response_received = true;
            tracing::info!("Face Identifier warmed up");
        }
        Ok(if self.is_done() { BrokerFlow::Break } else { BrokerFlow::Continue })
    }
}

impl Plan {
    /// Runs the warm up plan.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Started warming up");
        wait_until_cpu_is_not_overloaded(orb).await?; // to not further overload the system

        // Make sure all of our agents are reset before we re-spawn them. We need
        // this to ensure latest config updates are loaded properly.
        orb.mega_agent_one.kill().await;
        orb.mega_agent_two.kill().await;

        orb.enable_ir_net().await?;
        orb.enable_rgb_net(false).await?;

        let fence = Instant::now();
        self.ir_net_warmup(orb).await?;
        self.rgb_net_warmup(orb).await?;
        self.face_identifier_warmup(orb).await?;
        orb.run_with_fence(self, fence).await?;

        orb.disable_ir_net();
        orb.disable_rgb_net();

        wait_until_cpu_is_not_overloaded(orb).await?;
        tracing::info!("Finished warming up");
        Ok(())
    }

    /// Requests IR-Net estimate for a blank IR-camera frame.
    ///
    /// This method is useful for warming up IR-Net.
    ///
    /// # Panics
    ///
    /// If `mega_agent` is not enabled.
    async fn ir_net_warmup(&mut self, orb: &mut Orb) -> Result<()> {
        Ok(orb
            .mega_agent_one
            .enabled()
            .unwrap()
            .send(port::Input::new(mega_agent_one::Input::IRNet(ir_net::Input::Warmup)))
            .await?)
    }

    /// Requests RGB-Net estimate for a blank RGB-camera frame.
    ///
    /// This method is useful for warming up RGB-Net.
    ///
    /// # Panics
    ///
    /// If `mega_agent` is not enabled.
    async fn rgb_net_warmup(&mut self, orb: &mut Orb) -> Result<()> {
        Ok(orb
            .mega_agent_two
            .enabled()
            .unwrap()
            .send(port::Input::new(mega_agent_two::Input::RgbNet(rgb_net::Input::Warmup)))
            .await?)
    }

    /// Invoke face_identifier with a blank RGB-camera frame.
    ///
    /// This method is useful for warming up face_identifier.
    ///
    /// # Panics
    ///
    /// If `mega_agent` is not enabled.
    async fn face_identifier_warmup(&mut self, orb: &mut Orb) -> Result<()> {
        Ok(orb
            .mega_agent_two
            .enabled()
            .unwrap()
            .send(port::Input::new(mega_agent_two::Input::FaceIdentifier(
                face_identifier::Input::Warmup,
            )))
            .await?)
    }

    fn is_done(&self) -> bool {
        self.ir_net_estimate_received
            && self.rgb_net_estimate_received
            && self.face_identifier_response_received
    }
}

async fn wait_until_cpu_is_not_overloaded(orb: &mut Orb) -> Result<()> {
    if let Some(monitor::cpu::Report { cpu_load, .. }) = orb.cpu_monitor.last_report()? {
        if *cpu_load < CPU_OVERLOAD_THRESHOLD {
            tracing::info!("CPU load is below threshold: {cpu_load:.2}");
            return Ok(());
        }
    }
    let start_time = Instant::now();
    while let Some(monitor::cpu::Report { cpu_load, .. }) = orb.cpu_monitor.next().await {
        if cpu_load < CPU_OVERLOAD_THRESHOLD {
            let elapsed_time = start_time.elapsed().as_millis();
            tracing::info!("CPU load fell below threshold after {elapsed_time}ms: {cpu_load:.2}");
            return Ok(());
        }
    }
    bail!("CPU monitor failed");
}
