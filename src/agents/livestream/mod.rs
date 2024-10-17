//! Live-streaming over the network.
//!
//! Used for live-streaming data from camera sensors.

mod app;
mod downstream;
mod gpu;
mod upstream;

use self::{
    downstream::Downstream,
    gpu::Gpu,
    upstream::{Event, Upstream},
};
use crate::agents::{camera, camera::Frame, mirror, python, qr_code};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::{
    future::{self, Either},
    prelude::*,
};
use std::{convert::Infallible, sync::Arc, task::Poll};
use tokio::runtime;

/// Live-streaming agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Debug)]
pub struct Agent;

/// Input data to render on the livestream.
#[derive(Debug)]
pub enum Input {
    /// Clear the screen.
    Clear,
    /// Current phase.
    Phase(&'static str),
    /// Whether the IR eye camera is capturing.
    IrEyeState(bool),
    /// IR eye camera frame.
    IrEyeFrame(camera::ir::Frame),
    /// Whether the IR face camera is capturing.
    IrFaceState(bool),
    /// IR face camera frame.
    IrFaceFrame(camera::ir::Frame),
    /// Whether the RGB camera is capturing.
    RgbState(bool),
    /// RGB camera frame.
    RgbFrame(camera::rgb::Frame),
    /// Whether the thermal camera is capturing.
    ThermalState(bool),
    /// Thermal camera frame.
    ThermalFrame(camera::thermal::Frame),
    /// Whether the depth camera is capturing.
    DepthState(bool),
    /// Depth camera frame.
    DepthFrame(camera::depth::Frame),
    /// IR Net estimation.
    IrNetEstimate(python::ir_net::EstimateOutput),
    /// RGB Net estimation.
    RgbNetEstimate(python::rgb_net::EstimateOutput),
    /// Mirror set point.
    SetMirrorPoint(mirror::Point),
    /// QR-code.
    QrCode(qr_code::Points),
    /// Liquid lens setting.
    Focus(i16),
    /// IR camera exposure.
    Exposure(u16),
    /// Target the left eye if `true` or the right eye if `false`.
    TargetLeftEye(bool),
}

impl Port for Agent {
    type Input = Input;
    type Output = Infallible;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "livestream";
}

impl agentwire::agent::Thread for Agent {
    type Error = Error;

    #[allow(clippy::too_many_lines)]
    fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        let mut upstream = rt.block_on(Upstream::new())?;
        let mut downstream = None;
        let mut gpu = rt.block_on(Gpu::new())?;
        gpu.clear_textures();
        loop {
            let input = rt.block_on(future::poll_fn(|cx| {
                if let Poll::Ready(input) = port.poll_next_unpin(cx) {
                    return Poll::Ready(Either::Left(input));
                }
                if let Poll::Ready(event) = upstream.poll_next_unpin(cx) {
                    return Poll::Ready(Either::Right(event));
                }
                Poll::Pending
            }));
            let mut events = Vec::new();
            match input {
                Either::Left(None) | Either::Right(None) => break,
                Either::Right(Some(Err(err))) => {
                    tracing::error!("Livestream upstream error: {err}");
                    continue;
                }
                Either::Left(Some(input)) => match input.value {
                    Input::Clear => {
                        gpu.clear_textures();
                        gpu.app.clear();
                    }
                    Input::IrEyeFrame(frame) => {
                        gpu.update_camera_ir_eye(&frame);
                    }
                    Input::IrFaceFrame(frame) => {
                        gpu.update_camera_ir_face(&frame);
                    }
                    Input::RgbFrame(frame) => {
                        gpu.update_camera_rgb(frame.as_bytes(), frame.width(), frame.height());
                    }
                    Input::ThermalFrame(frame) => {
                        gpu.update_camera_thermal(&frame);
                    }
                    Input::DepthFrame(frame) => {
                        gpu.update_camera_depth(&frame);
                    }
                    Input::Phase(name) => {
                        gpu.app.set_phase(name);
                    }
                    Input::IrEyeState(ir_eye_state) => {
                        gpu.app.set_ir_eye_state(ir_eye_state);
                        continue;
                    }
                    Input::IrFaceState(ir_face_state) => {
                        gpu.app.set_ir_face_state(ir_face_state);
                        continue;
                    }
                    Input::RgbState(rgb_state) => {
                        gpu.app.set_rgb_state(rgb_state);
                        continue;
                    }
                    Input::ThermalState(thermal_state) => {
                        gpu.app.set_thermal_state(thermal_state);
                        continue;
                    }
                    Input::DepthState(depth_state) => {
                        gpu.app.set_depth_state(depth_state);
                        continue;
                    }
                    Input::IrNetEstimate(ir_net_estimate) => {
                        gpu.app.set_ir_net_estimate(ir_net_estimate);
                        continue;
                    }
                    Input::RgbNetEstimate(rgb_net_estimate) => {
                        gpu.app.set_rgb_net_estimate(rgb_net_estimate);
                        continue;
                    }
                    Input::SetMirrorPoint(point) => {
                        gpu.app.set_mirror_point(point);
                        continue;
                    }
                    Input::QrCode(points) => {
                        gpu.app.set_qr_code_points(points);
                        continue;
                    }
                    Input::Focus(focus) => {
                        gpu.app.set_ir_focus(focus);
                        continue;
                    }
                    Input::Exposure(exposure) => {
                        gpu.app.set_ir_exposure(exposure);
                        continue;
                    }
                    Input::TargetLeftEye(target_left_eye) => {
                        gpu.app.set_target_left_eye(target_left_eye);
                        continue;
                    }
                },
                Either::Right(Some(Ok(Event::Connected(addr)))) => {
                    tracing::info!("Accepted a new Livestream connection from {}", addr.ip());
                    downstream = Some(Arc::new(Downstream::new(addr.ip())?));
                }
                Either::Right(Some(Ok(Event::Closed))) => {
                    tracing::info!("Livestream connection closed by client");
                    downstream = None;
                }
                Either::Right(Some(Ok(Event::UiEvents(ui_events)))) => {
                    events = ui_events.into_iter().map(Into::into).collect();
                }
            };
            if let Some(downstream) = &downstream {
                let downstream = Arc::clone(downstream);
                gpu.render(events, move |buffer| downstream.push(&buffer));
            }
        }
        Ok(())
    }
}
