//! Thermal camera sensor.
//!
//! The Seek Thermal thermographic camera is connected via USB and provides
//! grayscale images of infrared radiation.

use super::FrameResolution;
use crate::{
    agents::ProcessInitializer,
    config::Config,
    consts::{CONFIG_DIR, THERMAL_HEIGHT, THERMAL_WIDTH},
    identification,
};
use agentwire::{
    agent,
    port::{self, Port, SharedPort},
};
use eyre::{Error, Result, WrapErr};
use orb_seekcamera::{Camera, Rotation};
use png::EncodingError;
use rkyv::{Archive, Deserialize, Infallible, Serialize};
use std::{
    env,
    io::prelude::*,
    mem::size_of,
    ops::Deref,
    sync::Arc,
    time::{Duration, SystemTime},
};

/// Thermal camera sensor.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Sensor {
    pairing_status_timeout: Duration,
}

/// Thermal camera frame.
///
/// This structure wraps the frame data into [`Arc`] inside, so the cloning is
/// cheap.
#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Frame(Arc<orb_seekcamera::Frame>);

/// Sensor commands.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Command {
    /// Start frame capturing.
    Start,
    /// Stop frame capturing.
    Stop,
    /// Creates a new Flat Scene Correction (FSC).
    FscCalibrate,
}

impl Port for Sensor {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 10;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Sensor {
    const SERIALIZED_INIT_SIZE: usize =
        size_of::<usize>() + size_of::<<Sensor as Archive>::Archived>();
    const SERIALIZED_INPUT_SIZE: usize = 128;
    const SERIALIZED_OUTPUT_SIZE: usize = 128 + THERMAL_HEIGHT as usize * THERMAL_WIDTH as usize;
}

impl agentwire::Agent for Sensor {
    const NAME: &'static str = "thermal-camera";
}

impl agentwire::agent::Process for Sensor {
    type Error = Error;

    fn run(self, mut port: port::RemoteInner<Self>) -> Result<(), Self::Error> {
        // Default behavior logs to file, we log to stdout/err instead.
        env::set_var("SEEKTHERMAL_LOG_STDOUT", "1");
        env::set_var("SEEKTHERMAL_LOG_STDERR", "1");
        env::set_var("SEEKTHERMAL_ROOT", CONFIG_DIR);
        let rotation = if identification::HARDWARE_VERSION.contains("Diamond") {
            Rotation::CounterClockwise
        } else {
            Rotation::Clockwise
        };
        let camera = match Camera::attach(self.pairing_status_timeout, rotation) {
            Ok(camera) => camera,
            Err(err) => {
                tracing::error!("Error connecting to the thermal camera: {err}");
                loop {
                    let command: Command = port.recv().value.deserialize(&mut Infallible).unwrap();
                    tracing::warn!("Ignoring thermal camera command: {command:?}");
                }
            }
        };
        loop {
            loop {
                match port.recv().value.deserialize(&mut Infallible).unwrap() {
                    Command::Start => break,
                    Command::Stop | Command::FscCalibrate => {}
                }
            }
            camera.capture_start()?;
            loop {
                let frame = camera.recv()?;
                port.try_send(&port::Output::new(Frame(Arc::new(frame))));
                if let Some(command) = port.try_recv() {
                    match command.value.deserialize(&mut Infallible).unwrap() {
                        Command::Start => {}
                        Command::Stop => break,
                        Command::FscCalibrate => camera.store_flat_scene_correction()?,
                    }
                }
            }
            camera.capture_stop()?;
        }
    }

    fn initializer() -> impl agent::process::Initializer {
        ProcessInitializer::default()
    }
}

impl From<&Config> for Sensor {
    fn from(config: &Config) -> Self {
        Self { pairing_status_timeout: config.thermal_camera_pairing_status_timeout }
    }
}

impl Deref for Frame {
    type Target = orb_seekcamera::Frame;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self(Arc::new(orb_seekcamera::Frame::new(
            vec![0; THERMAL_WIDTH as usize * THERMAL_HEIGHT as usize],
            Duration::default(),
            THERMAL_WIDTH as _,
            THERMAL_HEIGHT as _,
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
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.0)?;
        Ok(())
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.data()
    }

    fn timestamp(&self) -> Duration {
        self.0.timestamp()
    }

    #[allow(clippy::cast_possible_truncation)]
    fn width(&self) -> u32 {
        self.0.width() as _
    }

    #[allow(clippy::cast_possible_truncation)]
    fn height(&self) -> u32 {
        self.0.height() as _
    }
}

impl Frame {
    /// Decodes a PNG image into a frame.
    pub fn read_png<R: Read>(reader: R) -> Result<Self> {
        let decoder = png::Decoder::new(reader);
        let (info, mut reader) =
            decoder.read_info().wrap_err("failed to decode image, maybe encrypted?")?;
        let mut buf = vec![0; info.buffer_size()];
        reader.next_frame(&mut buf)?;
        Ok(Self(Arc::new(orb_seekcamera::Frame::new(
            buf,
            // Note: we may consider using the file modified time here instead
            SystemTime::UNIX_EPOCH.elapsed().unwrap_or(Duration::MAX),
            info.width as usize,
            info.height as usize,
        ))))
    }
}
