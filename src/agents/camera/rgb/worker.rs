//! RGB camera worker process.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera::{self, FrameResolution},
        Agent, AgentProcess, AgentProcessExitStrategy,
    },
    consts::{
        RGB_DEFAULT_HEIGHT, RGB_DEFAULT_WIDTH, RGB_EXPOSURE_RANGE, RGB_FPS, RGB_NATIVE_HEIGHT,
        RGB_NATIVE_WIDTH, RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH,
    },
    fisheye::{self, Fisheye},
    port::{self, Port, RemoteInner, SharedPort},
};
use eyre::{bail, eyre, Result, WrapErr};
use gstreamer::{
    buffer::{MappedBuffer, Readable},
    prelude::*,
    Caps, ElementFactory, Fraction, Pipeline,
};
use gstreamer_app::AppSink;
use ndarray::prelude::*;
use opencv::{
    core::{Mat_AUTO_STEP, Size, CV_8UC3},
    imgproc::{resize, INTER_LINEAR},
    prelude::*,
};
use png::EncodingError;
use rkyv::{Archive, Deserialize, Infallible, Serialize};
use std::{
    fmt,
    io::prelude::*,
    mem,
    ops::Deref,
    slice,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// RGB camera worker process.
#[derive(Default, Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Worker;

/// RGB camera frame.
///
/// This structure wraps the frame data into [`Arc`] inside, so the cloning is
/// cheap.
#[derive(Clone, Archive, Serialize, Deserialize)]
pub struct Frame {
    data: Arc<Vec<u8>>,
    timestamp: Duration,
    width: u32,
    height: u32,
}

/// RGB camera command.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Command {
    /// Play the pipeline.
    Play,
    /// Pause the pipeline. Some frames can be leaked into the next playing
    /// state.
    Pause,
    /// Reset the pipeline. This clears all internal queues and resets the auto-
    /// exposure.
    Reset,
    /// Fisheye configuration change.
    FisheyeConfig(Option<fisheye::Config>),
}

struct Stream {
    pipeline: Pipeline,
    appsink: AppSink,
}

impl Port for Worker {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Worker {
    const SERIALIZED_CONFIG_EXTRA_SIZE: usize = 0;
    const SERIALIZED_INPUT_SIZE: usize = 4096;
    const SERIALIZED_OUTPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
}

impl Agent for Worker {
    const NAME: &'static str = "rgb-camera-worker";
}

impl AgentProcess for Worker {
    fn run(self, mut port: RemoteInner<Self>) -> Result<()> {
        let mut undistortion_enabled = false;
        let mut fisheye = apply_fisheye_config(fisheye::Config::default())?;
        let stream = Stream::new()?;
        'outer: loop {
            loop {
                match port.recv().value.deserialize(&mut Infallible).unwrap() {
                    Command::Play => {
                        break;
                    }
                    Command::Pause => {
                        bail!("gstreamer pipeline is not playing")
                    }
                    Command::Reset => {
                        tracing::debug!("RGB camera GStreamer resetted");
                        stream.pipeline.set_state(gstreamer::State::Null)?;
                    }
                    Command::FisheyeConfig(Some(fisheye_config)) => {
                        fisheye = apply_fisheye_config(fisheye_config)?;
                        undistortion_enabled = true;
                    }
                    Command::FisheyeConfig(None) => {
                        undistortion_enabled = false;
                    }
                }
            }
            stream.pipeline.set_state(gstreamer::State::Playing)?;
            let mut errors_count = 0;
            loop {
                match stream.appsink.pull_sample() {
                    Ok(sample) => {
                        errors_count = 0;
                        let source_ts = Instant::now();
                        let buffer = sample
                            .buffer_owned()
                            .ok_or_else(|| eyre!("unable to obtain sample buffer"))?;
                        let timestamp: Duration = buffer
                            .pts()
                            .ok_or_else(|| eyre!("unable to obtain buffer timestamp"))?
                            .into();
                        let data = buffer
                            .into_mapped_buffer_readable()
                            .map_err(|_| eyre!("unable to obtain readable mapped buffer"))?;
                        let mut frame =
                            Frame::new(&data, timestamp, RGB_NATIVE_WIDTH, RGB_NATIVE_HEIGHT);
                        if undistortion_enabled {
                            frame.undistort(&fisheye)?;
                        }
                        port.try_send(port::Output { value: frame, source_ts });
                        if let Some(command) = port.try_recv() {
                            match command.value.deserialize(&mut Infallible).unwrap() {
                                Command::Reset | Command::Play => {
                                    bail!("gstreamer pipeline is playing")
                                }
                                Command::Pause => {
                                    break;
                                }
                                Command::FisheyeConfig(Some(fisheye_config)) => {
                                    fisheye = apply_fisheye_config(fisheye_config)?;
                                    undistortion_enabled = true;
                                }
                                Command::FisheyeConfig(None) => {
                                    undistortion_enabled = false;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to pull sample from GStreamer: {err:?}");
                        errors_count += 1;
                        if errors_count > 100 {
                            break 'outer;
                        }
                    }
                }
            }
            stream.pipeline.set_state(gstreamer::State::Paused)?;
        }
        Ok(())
    }

    fn exit_strategy(_code: Option<i32>, _signal: Option<i32>) -> AgentProcessExitStrategy {
        // Always close the port. The top-level RGB camera agent will notice
        // this and run custom recovery logic.
        AgentProcessExitStrategy::Close
    }
}

fn apply_fisheye_config(fisheye_config: fisheye::Config) -> Result<Fisheye> {
    Fisheye::try_from(fisheye_config).wrap_err("failed constructing fisheye from fisheye config")
}

impl Stream {
    fn new() -> Result<Self> {
        let pipeline = Pipeline::new(Some("rgb-camera"));
        let nvarguscamerasrc = ElementFactory::make("nvarguscamerasrc").build()?;
        let nvvidconv = ElementFactory::make("nvvidconv").build()?;
        let videoconvert = ElementFactory::make("videoconvert").build()?;
        let appsink = AppSink::builder().build();
        pipeline.add_many(&[&nvarguscamerasrc, &nvvidconv, &videoconvert, appsink.upcast_ref()])?;
        nvarguscamerasrc.set_property_from_str(
            "exposuretimerange",
            &format!("{} {}", RGB_EXPOSURE_RANGE.start(), RGB_EXPOSURE_RANGE.end()),
        );
        nvarguscamerasrc.link_filtered(
            &nvvidconv,
            &Caps::builder("video/x-raw")
                .features(["memory:NVMM"])
                .field("width", i32::try_from(RGB_NATIVE_WIDTH)?)
                .field("height", i32::try_from(RGB_NATIVE_HEIGHT)?)
                .field("format", "NV12")
                .field("framerate", Fraction::new(RGB_FPS.try_into()?, 1))
                .build(),
        )?;
        nvvidconv.set_property_from_str("flip-method", "3");
        nvvidconv.link_filtered(
            &videoconvert,
            &Caps::builder("video/x-raw")
                .field("width", i32::try_from(RGB_NATIVE_WIDTH)?)
                .field("height", i32::try_from(RGB_NATIVE_HEIGHT)?)
                .field("format", "BGRx")
                .build(),
        )?;
        videoconvert.link_filtered(
            &appsink,
            &Caps::builder("video/x-raw").field("format", "RGB").build(),
        )?;
        appsink.set_wait_on_eos(true);
        appsink.set_drop(true);
        appsink.set_max_buffers(1);
        Ok(Stream { pipeline, appsink })
    }
}

impl camera::Frame for Frame {
    fn write_png<W: Write>(
        &self,
        writer: W,
        resolution: FrameResolution,
    ) -> Result<(), EncodingError> {
        assert!(self.width % (resolution as u32) == 0);
        assert!(self.height % (resolution as u32) == 0);
        let png_width = self.width / (resolution as u32);
        let png_height = self.height / (resolution as u32);
        let mut encoder = png::Encoder::new(writer, png_width, png_height);
        encoder.set_color(png::ColorType::RGB);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        let mut writer = writer.stream_writer();
        unsafe {
            let mut ptr = self.data.as_ptr();
            for _ in 0..png_height {
                for _ in 0..png_width {
                    let r = *ptr;
                    ptr = ptr.add(1);
                    let g = *ptr;
                    ptr = ptr.add(1);
                    let b = *ptr;
                    ptr = ptr.add(1 + ((resolution as usize) - 1) * 3);
                    writer.write_all(&[r, g, b])?;
                }
                ptr = ptr.add(self.width as usize * ((resolution as usize) - 1) * 3);
            }
        }
        writer.finish()?;
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
    pub fn new(
        data: &MappedBuffer<Readable>,
        timestamp: Duration,
        width: u32,
        height: u32,
    ) -> Self {
        Self { data: Arc::new(data.to_vec()), timestamp, width, height }
    }

    /// Creates a new frame from a vector.
    #[must_use]
    pub fn from_vec(data: Vec<u8>, timestamp: Duration, width: u32, height: u32) -> Self {
        Self { data: Arc::new(data), timestamp, width, height }
    }

    /// Decodes a PNG image into a frame.
    pub fn read_png<R: Read>(reader: R) -> Result<Self> {
        let decoder = png::Decoder::new(reader);
        let (info, mut reader) = decoder.read_info()?;
        let mut buf = vec![0; info.buffer_size()];
        reader.next_frame(&mut buf)?;
        Ok(Self {
            data: Arc::new(buf),
            timestamp: SystemTime::UNIX_EPOCH.elapsed().unwrap_or(Duration::MAX),
            width: info.width,
            height: info.height,
        })
    }

    pub(super) fn undistort(&mut self, fisheye: &Fisheye) -> Result<()> {
        assert_eq!(self.width, RGB_NATIVE_WIDTH);
        assert_eq!(self.height, RGB_NATIVE_HEIGHT);
        if fisheye.rgb_width == RGB_REDUCED_WIDTH && fisheye.rgb_height == RGB_REDUCED_HEIGHT {
            self.data = Arc::new(unsafe { native_to_reduced(&self.data) });
            self.width = RGB_REDUCED_WIDTH;
            self.height = RGB_REDUCED_HEIGHT;
        } else if fisheye.rgb_width == RGB_DEFAULT_WIDTH && fisheye.rgb_height == RGB_DEFAULT_HEIGHT
        {
            self.data = Arc::new(unsafe { native_to_default(&self.data) });
            self.width = RGB_DEFAULT_WIDTH;
            self.height = RGB_DEFAULT_HEIGHT;
        }
        let data = mem::take(&mut self.data);
        let width = self.width;
        let height = self.height;
        self.data = Arc::new(fisheye.undistort_image(&data, width, height)?);
        Ok(())
    }

    /// Converts this frame into an owned 3-dimensional array.
    #[must_use]
    pub fn into_ndarray(&self) -> Array3<u8> {
        Array::from_shape_vec((self.height as usize, self.width as usize, 3), (*self.data).clone())
            .unwrap()
    }

    /// Resize the frame in place by given fraction.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn resize(&mut self, fraction: f64) -> Result<()> {
        let src = unsafe {
            Mat::new_rows_cols_with_data(
                self.height as i32,
                self.width as i32,
                CV_8UC3,
                self.data.as_slice().as_ptr() as *mut _,
                Mat_AUTO_STEP,
            )?
        };
        let mut dst = Mat::default();
        resize(&src, &mut dst, Size::default(), fraction, fraction, INTER_LINEAR)?;
        let len = dst
            .mat_size()
            .iter()
            .map(|&dim| dim as usize)
            .chain([dst.channels() as usize])
            .product();
        let ptr = dst.ptr(0)?.cast::<u8>();
        let slice = unsafe { slice::from_raw_parts(ptr, len) };
        self.data = Arc::new(slice.to_vec());
        self.height = dst.rows() as u32;
        self.width = dst.cols() as u32;
        Ok(())
    }
}

impl ArchivedFrame {
    /// Converts this frame into an owned 3-dimensional array.
    pub fn into_ndarray(&self) -> Array3<u8> {
        let vec = (*self.data).deserialize(&mut Infallible).unwrap();
        Array::from_shape_vec((self.height as usize, self.width as usize, 3), vec).unwrap()
    }

    /// Returns frame width.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns frame height.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns frame data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
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
            data: Arc::new(vec![0; RGB_DEFAULT_WIDTH as usize * RGB_DEFAULT_HEIGHT as usize * 3]),
            timestamp: Duration::default(),
            width: RGB_DEFAULT_WIDTH,
            height: RGB_DEFAULT_HEIGHT,
        }
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("timestamp", &self.timestamp)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

unsafe fn native_to_reduced(data: &[u8]) -> Vec<u8> {
    let input = data.as_ptr();
    let mut output = vec![0; RGB_REDUCED_WIDTH as usize * RGB_REDUCED_HEIGHT as usize * 3];
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..RGB_REDUCED_HEIGHT as usize {
            let row_in = input.add(y * RGB_NATIVE_WIDTH as usize * 3 * 5);
            let row_out = output.as_mut_ptr().add(y * RGB_REDUCED_WIDTH as usize * 3);
            for x in 0..RGB_REDUCED_WIDTH as usize {
                let px_in = row_in.add(x * 3 * 5);
                let px_out = row_out.add(x * 3);
                *px_out = *px_in;
                *px_out.add(1) = *px_in.add(1);
                *px_out.add(2) = *px_in.add(2);
            }
        }
    }
    output
}

unsafe fn native_to_default(data: &[u8]) -> Vec<u8> {
    let input = data.as_ptr();
    let mut output = vec![0; RGB_DEFAULT_WIDTH as usize * RGB_DEFAULT_HEIGHT as usize * 3];
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..RGB_DEFAULT_HEIGHT as usize {
            let row_in = input.add(y * RGB_NATIVE_WIDTH as usize * 3 * 2);
            let row_out = output.as_mut_ptr().add(y * RGB_DEFAULT_WIDTH as usize * 3);
            for x in 0..RGB_DEFAULT_WIDTH as usize {
                let px_in = row_in.add(x * 3 * 2);
                let px_out = row_out.add(x * 3);
                *px_out = *px_in;
                *px_out.add(1) = *px_in.add(1);
                *px_out.add(2) = *px_in.add(2);
            }
        }
    }
    output
}
