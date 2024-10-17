//! Downstream for the video stream.

use eyre::Result;
use gstreamer::{prelude::*, Caps, ElementFactory, Pipeline, Sample};
use gstreamer_app::AppSink;

const PORT: u16 = 9200;

/// Downstream listener.
pub struct Downstream {
    pipeline: Pipeline,
    appsink: AppSink,
}

impl Downstream {
    /// Creates a new [`Downstream`].
    pub fn new() -> Result<Self> {
        let pipeline = Pipeline::new();
        let udpsrc = ElementFactory::make("udpsrc").build()?;
        let rtph264depay = ElementFactory::make("rtph264depay").build()?;
        let h264parse = ElementFactory::make("h264parse").build()?;
        let avdec_h264 = ElementFactory::make("avdec_h264").build()?;
        let videoconvert = ElementFactory::make("videoconvert").build()?;
        let appsink = AppSink::builder().build();
        pipeline.add_many([
            &udpsrc,
            &rtph264depay,
            &h264parse,
            &avdec_h264,
            &videoconvert,
            appsink.upcast_ref(),
        ])?;
        udpsrc.set_property_from_str("port", &PORT.to_string());
        udpsrc.link_filtered(&rtph264depay, &Caps::builder("application/x-rtp").build())?;
        rtph264depay.link(&h264parse)?;
        h264parse.link(&avdec_h264)?;
        avdec_h264.link(&videoconvert)?;
        videoconvert.link_filtered(
            &appsink,
            &Caps::builder("video/x-raw").field("format", "RGBx").build(),
        )?;
        Ok(Self { pipeline, appsink })
    }

    /// Starts the video stream pipeline.
    pub fn start(&self) -> Result<()> {
        self.pipeline.set_state(gstreamer::State::Playing)?;
        Ok(())
    }

    /// Pulls a sample from the video stream.
    pub fn pull_sample(&self) -> Result<Sample> {
        Ok(self.appsink.pull_sample()?)
    }
}
