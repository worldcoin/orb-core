//! Infra-red camera sensor.
//!
//! Sensor: [VC
//! MIPI](https://www.vision-components.com/en/products/oem-embedded-vision-systems/mipi-camera-modules/)
//! IMX392
//!
//! - 1/2.3&ldquo; CMOS sensor, monochrome, SONY Pregius
//! - 1920 x 1200 pixel, 2.3 MP
//! - 200 fps, global shutter
//! - trigger input/flash trigger output

// Experimentally probed list of supported formats:
// - V4L2_PIX_FMT_GREY
// - V4L2_PIX_FMT_Y10
// - V4L2_PIX_FMT_Y12
// - V4L2_PIX_FMT_YUYV
// - V4L2_PIX_FMT_YVYU
// - V4L2_PIX_FMT_UYVY
// - V4L2_PIX_FMT_VYUY
// - V4L2_PIX_FMT_NV16
// - V4L2_PIX_FMT_SRGGB8
// - V4L2_PIX_FMT_SBGGR10
// - V4L2_PIX_FMT_SGBRG10
// - V4L2_PIX_FMT_SGRBG10
// - V4L2_PIX_FMT_SRGGB10
// - V4L2_PIX_FMT_SGRBG12
// - V4L2_PIX_FMT_SRGGB12

use super::{frame_flip, frame_rotate_cw, frame_rotate_cw_flip, FrameResolution};
use crate::{
    consts::{
        IR_CAMERA_DEFAULT_BLACK_LEVEL, IR_CAMERA_DEFAULT_EXPOSURE, IR_CAMERA_DEFAULT_GAIN,
        IR_HEIGHT, IR_WIDTH,
    },
    ext::mpsc::SenderExt as _,
    logger::{LogOnError, DATADOG, NO_TAGS},
    poll_commands, port,
    port::Port,
    time_series::TimeSeries,
};
use eyre::{ensure, Result, WrapErr};
use futures::{
    channel::{mpsc, oneshot},
    prelude::*,
};
use ndarray::prelude::*;
use orb_camera::{Buffer, Device, Format};
use png::EncodingError;
use rkyv::{Archive, Deserialize, Infallible, Serialize};
use std::{
    convert::TryInto,
    fmt,
    io::prelude::*,
    ops::Deref,
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::{Duration, Instant, SystemTime},
};

const EYE_DEVICE_PATH: &str = "/dev/video0";
const FACE_DEVICE_PATH: &str = "/dev/video2";
const BUF_COUNT: u32 = 4;
const SLEEP_TIMEOUT: Duration = Duration::from_millis(100);
const PIX_FMT: u32 = v4l2_sys::V4L2_PIX_FMT_Y10;
const TRIGGER_MODE: i64 = 1; // mode 0: free run | mode 1: external trigger

/// Infra-red camera sensor.
///
/// See [the module-level documentation](self) for details.
#[derive(Debug)]
pub struct Sensor {
    state_tx: Option<mpsc::Sender<super::State>>,
    device_path: &'static str,
    rotation: bool,
}

impl Sensor {
    /// Initializes IR Eye Camera Sensor mounted in the back of the Orb.
    #[must_use]
    pub fn eye(state_tx: Option<mpsc::Sender<super::State>>) -> Self {
        Self { state_tx, device_path: EYE_DEVICE_PATH, rotation: false }
    }

    /// Initializes IR Face Camera Sensor mounted in the front of the Orb.
    #[must_use]
    pub fn face(state_tx: Option<mpsc::Sender<super::State>>) -> Self {
        Self { state_tx, device_path: FACE_DEVICE_PATH, rotation: true }
    }
}

/// Configuration history.
#[derive(Debug)]
pub struct Log {
    /// Gain parameter history.
    pub gain: TimeSeries<i64>,
    /// Exposure parameter history.
    pub exposure: TimeSeries<i64>,
    /// Framerate per second during last capture.
    pub fps: u64,
}

/// Sensor commands.
#[derive(Debug)]
pub enum Command {
    /// Start frame capturing.
    Start,
    /// Stop frame capturing.
    Stop(oneshot::Sender<Log>),
    /// Set camera gain.
    SetGain(i64),
    /// Set camera exposure.
    SetExposure(i64),
    /// Activate or deactivate frame flipping.
    SetFlip(bool),
    /// Set the Black level of the camera.
    SetBlackLevel(i64),
}

/// Sensor frame.
///
/// This structure wraps the frame data into [`Arc`] inside, so the cloning is
/// cheap.
#[derive(Clone, Archive, Serialize, Deserialize, Eq, PartialEq)]
pub struct Frame {
    data: Arc<Vec<u8>>,
    timestamp: Duration,
    width: u32,
    height: u32,
    mean: u8,
}

#[derive(Debug)]
struct PendingFrame<'a> {
    buf: &'a Buffer<'a>,
    buf_idx: u32,
    timestamp: Duration,
    format: &'a Format,
}

impl Port for Sensor {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 100;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Sensor {
    const NAME: &'static str = "ir-camera";
}

impl super::AgentThread for Sensor {
    #[allow(clippy::too_many_lines)]
    fn run(mut self, mut port: port::Inner<Self>) -> Result<()> {
        let mut restart = false;
        let mut flip = false;
        let mut exit = false;
        let mut scratch_buffer = Vec::<u16>::with_capacity((IR_WIDTH * IR_HEIGHT) as usize);
        while !exit {
            let sensor = Device::open(self.device_path)?;
            let mut format = sensor.format()?;
            format.pixel_format = PIX_FMT;
            format = sensor.set_format(&format)?;
            ensure!(format.pixel_format == PIX_FMT, "couldn't set pixel format");
            sensor.set_control("Trigger Mode", TRIGGER_MODE)?;
            sensor.set_control("Gain", IR_CAMERA_DEFAULT_GAIN)?;
            sensor.set_control("Exposure", IR_CAMERA_DEFAULT_EXPOSURE)?;
            sensor.set_control("Black Level", IR_CAMERA_DEFAULT_BLACK_LEVEL)?;
            let buf = Buffer::new(&sensor, BUF_COUNT)?;
            sensor.with_waiter_context(|waiter, cx| {
                'enable: loop {
                    if restart {
                        restart = false;
                    } else {
                        'start: loop {
                            poll_commands! { |port, cx|
                                Some(Command::SetGain(gain)) => sensor.set_control("Gain", gain)?,
                                Some(Command::SetExposure(exposure)) => sensor.set_control("Exposure", exposure)?,
                                Some(Command::SetFlip(new_flip)) => flip = new_flip,
                                Some(Command::SetBlackLevel(black_level)) => {
                                    sensor.set_control("Black Level", black_level)?;
                                }
                                Some(Command::Start) => break 'start,
                                None => {
                                    exit = true;
                                    break 'enable Ok(());
                                }
                            }
                            waiter.wait_event(SLEEP_TIMEOUT)?;
                        }
                    }
                    if let Some(state_tx) = &mut self.state_tx {
                        state_tx.send_now(super::State::Capturing)?;
                    }
                    for i in 0..buf.count() {
                        if let Err(err) = buf.enqueue(i) {
                            if let Some(state_tx) = &mut self.state_tx {
                                state_tx.send_now(super::State::Error)?;
                            }
                            DATADOG.incr("orb.main.count.hardware.camera.issue.ir_camera.initialization_problem_restarting", NO_TAGS).or_log();
                            tracing::error!("IR camera initialization error: {}. Restarting the camera.", err);
                            restart = true;
                            break 'enable Ok(());
                        }
                    }
                    sensor.start()?;
                    let capture_start_timestamp = Instant::now();
                    let mut log = Log::default();
                    let mut frame_counter: u64 = 0;
                    let mut latest_frame = None;
                    let mut latest_timestamp = SystemTime::now();
                    'capture: loop {
                        poll_commands! { |port, cx|
                            Some(Command::SetGain(gain)) => {
                                sensor.set_control("Gain", gain)?;
                                log.gain.push(gain);
                            }
                            Some(Command::SetExposure(exposure)) => {
                                sensor.set_control("Exposure", exposure)?;
                                log.exposure.push(exposure);
                            }
                            Some(Command::SetFlip(new_flip)) => {
                                flip = new_flip;
                            }
                            Some(Command::Stop(log_tx)) => {
                                let capture_time = capture_start_timestamp.elapsed().as_secs();
                                log.fps = if capture_time > 0 {
                                    frame_counter / capture_time
                                } else {
                                    0
                                };
                                tracing::info!("FPS of video device {}: {}", self.device_path, log.fps);
                                #[allow(let_underscore_drop)]
                                let _ = log_tx.send(log);
                                if let Some(state_tx) = &mut self.state_tx {
                                    state_tx.send_now(super::State::Idle)?;
                                }
                                break 'capture;
                            }
                            Some(Command::SetBlackLevel(black_level)) => {
                                sensor.set_control("Black Level", black_level)?;
                            }
                            None => {
                                exit = true;
                                break 'enable Ok(());
                            },
                        }
                        waiter.wait(SLEEP_TIMEOUT)?;
                        match buf.dequeue() {
                            Ok(Some(dequeued)) => {
                                let timestamp = dequeued.timestamp;
                                latest_frame = Some(PendingFrame::new(
                                    &buf,
                                    dequeued.index,
                                    timestamp,
                                    &format,
                                ));
                                let delay = latest_timestamp.elapsed().unwrap_or(Duration::MAX).as_millis();
                                DATADOG.timing("orb.main.time.camera.ir_frame", delay.try_into().unwrap_or(i64::MAX), NO_TAGS).or_log();
                                latest_timestamp = SystemTime::now();
                                frame_counter += 1;
                            }
                            Ok(None) => {} // woken by async context
                            Err(err) => {
                                if let Some(state_tx) = &mut self.state_tx {
                                    state_tx.send_now(super::State::Error)?;
                                }
                                tracing::error!("IR camera V4L buffer dequeue failed: {}", err);
                                restart = true;
                                break;
                            }
                        }
                        if latest_timestamp.elapsed().unwrap_or_default() > Duration::from_millis(500) {
                            if let Some(state_tx) = &mut self.state_tx {
                                state_tx.send_now(super::State::Error)?;
                            }
                            DATADOG.incr("orb.main.count.hardware.camera.issue.ir_camera.problem_restarting", NO_TAGS).or_log();
                            tracing::error!("IR camera doesn't work. Restarting.");
                            restart = true;
                            break;
                        }
                        if let Poll::Ready(ready) = Pin::new(&mut port).poll_ready(cx) {
                            ready?;
                            if let Some(frame) = latest_frame.take() {
                                let frame = frame.convert(flip, self.rotation, &mut scratch_buffer);
                                Pin::new(&mut port).start_send(port::Output::new(frame))?;
                            }
                        }
                    }
                    sensor.stop()?;
                    if restart {
                        break Ok(());
                    }
                }
            })??;
        }
        Ok(())
    }
}

impl port::Outer<Sensor> {
    /// Stops the capturing and returns configuration history log.
    pub async fn stop(&mut self) -> Result<Log> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Command::Stop(tx))).await?;
        Ok(rx.await?)
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            gain: TimeSeries::builder().limit(1_000_000).build(),
            exposure: TimeSeries::builder().limit(1_000_000).build(),
            fps: 0,
        }
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
        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.data)?;
        Ok(())
    }

    fn timestamp(&self) -> Duration {
        self.timestamp
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}

impl Frame {
    /// Creates a new frame.
    #[must_use]
    pub fn new(data: Vec<u8>, timestamp: Duration, width: u32, height: u32, mean: u8) -> Self {
        Self { data: Arc::new(data), timestamp, width, height, mean }
    }

    /// Decodes a PNG image into a frame.
    pub fn read_png<R: Read>(reader: R) -> Result<Self> {
        let decoder = png::Decoder::new(reader);
        let (info, mut reader) =
            decoder.read_info().wrap_err("failed to decode image, maybe encrypted?")?;
        let mut buf = vec![0; info.buffer_size()];
        reader.next_frame(&mut buf)?;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            clippy::cast_sign_loss
        )]
        let mean = (buf.iter().fold(0, |sum, &px| sum + u64::from(px)) / buf.len() as u64) as u8;
        Ok(Self::new(
            buf,
            // Note: we may consider using the file modified time here instead
            SystemTime::UNIX_EPOCH.elapsed().unwrap_or(Duration::MAX),
            info.width,
            info.height,
            mean,
        ))
    }

    /// Returns the frame mean value.
    #[must_use]
    pub fn mean(&self) -> u8 {
        self.mean
    }

    /// Converts this frame into an owned 3-dimensional array.
    #[must_use]
    pub fn into_ndarray(&self) -> Array2<u8> {
        Array::from_shape_vec((self.height as usize, self.width as usize), (*self.data).clone())
            .unwrap()
    }
}

impl ArchivedFrame {
    /// Converts this frame into an owned 2-dimensional array.
    pub fn into_ndarray(&self) -> Array2<u8> {
        let vec = (*self.data).deserialize(&mut Infallible).unwrap();
        Array::from_shape_vec((IR_HEIGHT as usize, IR_WIDTH as usize), vec).unwrap()
    }
}

impl Deref for Frame {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            data: Arc::new(vec![0; IR_WIDTH as usize * IR_HEIGHT as usize]),
            timestamp: Duration::default(),
            width: IR_WIDTH,
            height: IR_HEIGHT,
            mean: 0,
        }
    }
}

impl PendingFrame<'_> {
    fn convert(self, flip: bool, rotation: bool, buf: &mut Vec<u16>) -> Frame {
        let src = self.buf.get(self.buf_idx);
        unsafe {
            buf.set_len(src.len() / 2);
            // memcpy-ing the source into an intermediate buffer significantly improves performance.
            std::ptr::copy_nonoverlapping(src.as_ptr(), buf.as_mut_ptr().cast::<u8>(), src.len());
        }
        #[allow(clippy::cast_possible_truncation)]
        let (dst, width, height) = {
            let (width, height) = (self.format.width as usize, self.format.height as usize);
            match (rotation, flip) {
                (true, true) => (
                    frame_rotate_cw_flip(buf, width, height, convert_10_as_16_bit_to_8_bit),
                    height as u32,
                    width as u32,
                ),
                (true, false) => (
                    frame_rotate_cw(buf, width, height, convert_10_as_16_bit_to_8_bit),
                    height as u32,
                    width as u32,
                ),
                (false, true) => (
                    frame_flip(buf, width, height, convert_10_as_16_bit_to_8_bit),
                    width as u32,
                    height as u32,
                ),
                (false, false) => (
                    buf.iter().map(|&px| convert_10_as_16_bit_to_8_bit(px)).collect(),
                    width as u32,
                    height as u32,
                ),
            }
        };
        #[allow(clippy::cast_possible_truncation)]
        let mean = {
            let sum: u64 = dst.iter().map(|&px| u64::from(px)).sum();
            (sum / dst.len() as u64) as u8
        };
        DATADOG.gauge("orb.main.gauge.camera.ir_camera.mean", mean.to_string(), NO_TAGS).or_log();
        Frame::new(dst, self.timestamp, width, height, mean)
    }
}

/// Converts a pixel from 10-bit in 16-bit format to 8-bit gray value by
/// removing the lowermost bits.
#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn convert_10_as_16_bit_to_8_bit(px: u16) -> u8 {
    // Pixel mask: 0b1111111_11000000
    // Added later: the above doesn't work. 7 is an experimentally found value.
    (px >> 7) as u8
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("timestamp", &self.timestamp)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("mean", &self.mean)
            .finish_non_exhaustive()
    }
}

impl<'a> PendingFrame<'a> {
    fn new(buf: &'a Buffer<'a>, buf_idx: u32, timestamp: Duration, format: &'a Format) -> Self {
        Self { buf, buf_idx, timestamp, format }
    }
}

impl Drop for PendingFrame<'_> {
    fn drop(&mut self) {
        self.buf.enqueue(self.buf_idx).expect("buffer enqueue failed");
    }
}
