//! RGB camera worker process.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera::{self, Frame as _, FrameResolution},
        ProcessInitializer,
    },
    consts::{
        RGB_DEFAULT_HEIGHT, RGB_DEFAULT_WIDTH, RGB_EXPOSURE_RANGE, RGB_FPS, RGB_NATIVE_HEIGHT,
        RGB_NATIVE_WIDTH, RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH,
    },
    image::fisheye::{self, Fisheye},
};
use agentwire::{
    agent,
    port::{self, Port, RemoteInner, SharedPort},
};
use eyre::{bail, eyre, Error, Result, WrapErr};
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
use rkyv::{
    ser::Serializer,
    vec::{ArchivedVec, RawArchivedVec, VecResolver},
    Archive, Deserialize, Fallible, Infallible, Serialize,
};
use std::{
    fmt,
    io::prelude::*,
    mem::{size_of, take},
    ptr::copy_nonoverlapping,
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
    data: Arc<FrameData>,
    timestamp: Duration,
    width: u32,
    height: u32,
}

enum FrameData {
    Owned(Vec<u8>),
    Mapped(MappedBuffer<Readable>),
}

/// RGB camera command.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Command {
    /// Play the pipeline.
    Play(u32),
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
    const SERIALIZED_INIT_SIZE: usize =
        size_of::<usize>() + size_of::<<Worker as Archive>::Archived>();
    const SERIALIZED_INPUT_SIZE: usize = 4096;
    const SERIALIZED_OUTPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
}

impl agentwire::Agent for Worker {
    const NAME: &'static str = "rgb-camera-worker";
}

impl agentwire::agent::Process for Worker {
    type Error = Error;

    fn run(self, mut port: RemoteInner<Self>) -> Result<(), Self::Error> {
        let mut undistortion_enabled = false;
        let mut fisheye = apply_fisheye_config(fisheye::Config::default())?;
        let mut prev_fps = RGB_FPS;
        let mut stream = Stream::new(prev_fps)?;
        'outer: loop {
            let fps = loop {
                match port.recv().value.deserialize(&mut Infallible).unwrap() {
                    Command::Play(fps) => {
                        break fps;
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
            };
            if fps != prev_fps {
                stream = Stream::new(fps)?;
                prev_fps = fps;
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
                            Frame::new(data, timestamp, RGB_NATIVE_WIDTH, RGB_NATIVE_HEIGHT);
                        if undistortion_enabled {
                            frame.undistort(&fisheye)?;
                        }
                        port.try_send(&port::Output { value: frame, source_ts });
                        if let Some(command) = port.try_recv() {
                            match command.value.deserialize(&mut Infallible).unwrap() {
                                Command::Reset | Command::Play(_) => {
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
                        if errors_count > 50 {
                            break 'outer;
                        }
                    }
                }
            }
            stream.pipeline.set_state(gstreamer::State::Paused)?;
        }
        Ok(())
    }

    fn exit_strategy(_code: Option<i32>, _signal: Option<i32>) -> agent::process::ExitStrategy {
        // Always close the port. The top-level RGB camera agent will notice
        // this and run custom recovery logic.
        agent::process::ExitStrategy::Close
    }

    fn initializer() -> impl agent::process::Initializer {
        ProcessInitializer::default()
    }
}

fn apply_fisheye_config(fisheye_config: fisheye::Config) -> Result<Fisheye> {
    Fisheye::try_from(fisheye_config).wrap_err("failed constructing fisheye from fisheye config")
}

impl Stream {
    fn new(fps: u32) -> Result<Self> {
        let pipeline = Pipeline::with_name("rgb-camera");
        let nvarguscamerasrc = ElementFactory::make("nvarguscamerasrc").build()?;
        let nvvidconv = ElementFactory::make("nvvidconv").build()?;
        let videoconvert = ElementFactory::make("videoconvert").build()?;
        let appsink = AppSink::builder().build();
        pipeline.add_many([&nvarguscamerasrc, &nvvidconv, &videoconvert, appsink.upcast_ref()])?;
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
                .field("framerate", Fraction::new(fps.try_into()?, 1))
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
            let mut ptr = self.as_bytes().as_ptr();
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

    fn as_bytes(&self) -> &[u8] {
        self.data.as_slice()
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
    pub fn new(data: MappedBuffer<Readable>, timestamp: Duration, width: u32, height: u32) -> Self {
        Self { data: Arc::new(FrameData::Mapped(data)), timestamp, width, height }
    }

    /// Creates a new frame from a vector.
    #[must_use]
    pub fn from_vec(data: Vec<u8>, timestamp: Duration, width: u32, height: u32) -> Self {
        Self { data: Arc::new(FrameData::Owned(data)), timestamp, width, height }
    }

    /// Decodes a PNG image into a frame.
    pub fn read_png<R: Read>(reader: R) -> Result<Self> {
        let decoder = png::Decoder::new(reader);
        let (info, mut reader) = decoder.read_info()?;
        let mut buf = vec![0; info.buffer_size()];
        reader.next_frame(&mut buf)?;
        Ok(Self {
            data: Arc::new(FrameData::Owned(buf)),
            timestamp: SystemTime::UNIX_EPOCH.elapsed().unwrap_or(Duration::MAX),
            width: info.width,
            height: info.height,
        })
    }

    pub(super) fn undistort(&mut self, fisheye: &Fisheye) -> Result<()> {
        assert_eq!(self.width, RGB_NATIVE_WIDTH);
        assert_eq!(self.height, RGB_NATIVE_HEIGHT);
        if fisheye.rgb_width == RGB_REDUCED_WIDTH && fisheye.rgb_height == RGB_REDUCED_HEIGHT {
            self.data = Arc::new(FrameData::Owned(unsafe { native_to_reduced(self.as_bytes()) }));
            self.width = RGB_REDUCED_WIDTH;
            self.height = RGB_REDUCED_HEIGHT;
        } else if fisheye.rgb_width == RGB_DEFAULT_WIDTH && fisheye.rgb_height == RGB_DEFAULT_HEIGHT
        {
            self.data = Arc::new(FrameData::Owned(unsafe { native_to_default(self.as_bytes()) }));
            self.width = RGB_DEFAULT_WIDTH;
            self.height = RGB_DEFAULT_HEIGHT;
        }
        let data = take(&mut self.data);
        let width = self.width;
        let height = self.height;
        self.data =
            Arc::new(FrameData::Owned(fisheye.undistort_image(data.as_slice(), width, height)?));
        Ok(())
    }

    /// Converts this frame into an owned 3-dimensional array.
    #[must_use]
    pub fn into_ndarray(&self) -> Array3<u8> {
        Array::from_shape_vec(
            (self.height as usize, self.width as usize, 3),
            self.as_bytes().to_vec(),
        )
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
                self.as_bytes().as_ptr() as *mut _,
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
        self.data = Arc::new(FrameData::Owned(slice.to_vec()));
        self.height = dst.rows() as u32;
        self.width = dst.cols() as u32;
        Ok(())
    }
}

impl ArchivedFrame {
    /// Converts this frame into an owned 3-dimensional array.
    pub fn into_ndarray(&self) -> Array3<u8> {
        let data = (*self.data).deserialize(&mut Infallible).unwrap();
        let FrameData::Owned(data) = data else { panic!("deserialized into a non-owned variant") };
        Array::from_shape_vec((self.height as usize, self.width as usize, 3), data).unwrap()
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

impl Default for Frame {
    fn default() -> Self {
        Self {
            data: Arc::new(FrameData::default()),
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

impl FrameData {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(data) => data.as_slice(),
            Self::Mapped(data) => data.as_slice(),
        }
    }
}

impl Default for FrameData {
    fn default() -> Self {
        Self::Owned(vec![0; RGB_DEFAULT_WIDTH as usize * RGB_DEFAULT_HEIGHT as usize * 3])
    }
}

impl Archive for FrameData {
    type Archived = RawArchivedVec<u8>;
    type Resolver = VecResolver;

    unsafe fn resolve(&self, pos: usize, resolver: Self::Resolver, out: *mut Self::Archived) {
        unsafe { RawArchivedVec::resolve_from_slice(self.as_slice(), pos, resolver, out) };
    }
}

impl<S> Serialize<S> for FrameData
where
    S: Serializer,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        unsafe { ArchivedVec::serialize_copy_from_slice(self.as_slice(), serializer) }
    }
}

impl<D> Deserialize<FrameData, D> for RawArchivedVec<u8>
where
    D: Fallible + ?Sized,
{
    fn deserialize(&self, _: &mut D) -> Result<FrameData, D::Error> {
        let mut result = Vec::with_capacity(self.len());
        unsafe {
            copy_nonoverlapping(self.as_ptr().cast(), result.as_mut_ptr(), self.len());
            result.set_len(self.len());
        }
        Ok(FrameData::Owned(result))
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
