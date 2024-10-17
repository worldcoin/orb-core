//! 2D Time-of-Flight camera sensor.

#![cfg_attr(not(all(target_arch = "aarch64", target_os = "linux")), allow(unused_imports))]

use super::FrameResolution;
use crate::{
    consts::{
        DEPTH_EXPOSURE_MANUAL, DEPTH_EXPOSURE_TIME, DEPTH_HEIGHT, DEPTH_USE_CASE, DEPTH_WIDTH,
    },
    ext::mpsc::{ReceiverExt as _, SenderExt as _},
};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::prelude::*;
use png::EncodingError;
use std::{io::prelude::*, ops::Deref, slice, sync::Arc, time::Duration};
use tokio::runtime;

/// 2D ToF camera sensor.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Default, Debug)]
pub struct Sensor;

/// Sensor commands.
#[derive(Debug)]
pub enum Command {
    /// Start frame capturing.
    Start,
    /// Stop frame capturing.
    Stop,
}

/// 2D ToF camera frame.
///
/// This structure wraps the frame data into [`Arc`] inside, so the cloning is
/// cheap.
#[derive(Clone, Debug)]
pub struct Frame(Arc<orb_royale::Frame>);

impl Port for Sensor {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 10;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Sensor {
    const NAME: &'static str = "depth-camera";
}

impl agentwire::agent::Thread for Sensor {
    type Error = Error;

    fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        {
            let camera = match orb_royale::Camera::attach() {
                Ok(camera) => camera,
                Err(err) => {
                    tracing::error!("Error connecting to the depth camera: {err}");
                    while let Some(input) = rt.block_on(port.next()) {
                        tracing::warn!("Ignoring depth camera command: {:?}", input.value);
                    }
                    return Ok(());
                }
            };
            loop {
                loop {
                    let Some(input) = rt.block_on(port.next()) else { return Ok(()) };
                    match input.value {
                        Command::Start => break,
                        Command::Stop => {}
                    }
                }
                let use_cases = camera.get_use_cases()?;
                tracing::debug!("Depth camera supported use cases: {use_cases:?}");
                camera.set_use_case(DEPTH_USE_CASE)?;
                camera.set_exposure_mode(DEPTH_EXPOSURE_MANUAL)?;
                camera.set_exposure_time(DEPTH_EXPOSURE_TIME)?;
                let frame_rate = camera.get_frame_rate()?;
                let max_frame_rate = camera.get_max_frame_rate()?;
                let exposure_limits = camera.get_exposure_limits()?;
                let is_manual = camera.is_exposure_mode_manual()?;
                tracing::info!(
                    "Depth camera frame rate: {frame_rate} FPS (max: {max_frame_rate} FPS), \
                     exposure limits: {}..{}, exposure mode is {}",
                    exposure_limits[0],
                    exposure_limits[1],
                    if is_manual { "manual" } else { "automatic" }
                );
                camera.capture_start()?;
                loop {
                    let frame = camera.recv();
                    port.tx.send_now(port::Output::new(Frame(Arc::new(frame))))?;
                    let Ok(command) = port.rx.try_recv() else { return Ok(()) };
                    if let Some(command) = command {
                        match command.value {
                            Command::Start => {}
                            Command::Stop => break,
                        }
                    }
                }
                camera.capture_stop()?;
            }
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
        {
            while rt.block_on(port.next()).is_some() {}
            Ok(())
        }
    }
}

impl Deref for Frame {
    type Target = orb_royale::Frame;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for Frame {
    #[allow(clippy::cast_possible_truncation)]
    fn default() -> Self {
        Self(Arc::new(orb_royale::Frame::new(
            vec![orb_royale::DepthPoint::default(); DEPTH_WIDTH as usize * DEPTH_HEIGHT as usize],
            vec![0; DEPTH_WIDTH as usize * DEPTH_HEIGHT as usize],
            Duration::default(),
            DEPTH_WIDTH as _,
            DEPTH_HEIGHT as _,
        )))
    }
}

impl super::Frame for Frame {
    fn write_png<W: Write>(
        &self,
        writer: W,
        resolution: FrameResolution,
    ) -> Result<(), EncodingError> {
        match resolution {
            FrameResolution::MAX => {}
            FrameResolution::MEDIUM | FrameResolution::LOW => unimplemented!(),
        }
        let mut encoder = png::Encoder::new(writer, self.width(), self.height());
        encoder.set_color(png::ColorType::RGB);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        let mut writer = writer.stream_writer();
        for point in &***self {
            writer.write_all(&point.to_rgb())?;
        }
        writer.finish()?;
        Ok(())
    }

    fn as_bytes(&self) -> &[u8] {
        let half_word_slice = self.as_gray();
        let ptr = half_word_slice.as_ptr().cast();
        let len = half_word_slice.len().checked_mul(2).unwrap();
        unsafe { slice::from_raw_parts(ptr, len) }
    }

    fn timestamp(&self) -> Duration {
        self.0.timestamp()
    }

    fn width(&self) -> u32 {
        u32::from(self.0.width())
    }

    fn height(&self) -> u32 {
        u32::from(self.0.height())
    }
}
