use crate::consts::{LIVESTREAM_FRAME_HEIGHT, LIVESTREAM_FRAME_WIDTH};
use eyre::Result;
use gstreamer::{prelude::*, ElementFactory, Pipeline};
use gstreamer_app::AppSrc;
use gstreamer_video::{VideoFormat, VideoFrameRef, VideoInfo};
use std::{net::IpAddr, ptr};

const PORT: u16 = 9200;

pub struct Downstream {
    pipeline: Pipeline,
    appsrc: AppSrc,
    video_info: VideoInfo,
}

impl Downstream {
    #[allow(clippy::cast_possible_truncation)]
    pub fn new(addr: IpAddr) -> Result<Self> {
        let video_info =
            VideoInfo::builder(VideoFormat::Bgrx, LIVESTREAM_FRAME_WIDTH, LIVESTREAM_FRAME_HEIGHT)
                .build()?;
        let pipeline = Pipeline::with_name("livestream");
        let appsrc = AppSrc::builder().caps(&video_info.to_caps()?).build();
        let nvvidconv = ElementFactory::make("nvvidconv").build()?;
        let nvv4l2h264enc = ElementFactory::make("nvv4l2h264enc").build()?;
        let rtph264pay = ElementFactory::make("rtph264pay").build()?;
        let udpsink = ElementFactory::make("udpsink").build()?;
        pipeline.add_many([
            appsrc.upcast_ref(),
            &nvvidconv,
            &nvv4l2h264enc,
            &rtph264pay,
            &udpsink,
        ])?;
        nvv4l2h264enc.set_property_from_str("insert-sps-pps", "1");
        nvv4l2h264enc.set_property_from_str("insert-vui", "1");
        udpsink.set_property_from_str("host", &addr.to_string());
        udpsink.set_property_from_str("port", &PORT.to_string());
        appsrc.link(&nvvidconv)?;
        nvvidconv.link(&nvv4l2h264enc)?;
        nvv4l2h264enc.link(&rtph264pay)?;
        rtph264pay.link(&udpsink)?;
        pipeline.set_state(gstreamer::State::Playing)?;
        appsrc.set_block(true);
        Ok(Self { pipeline, appsrc, video_info })
    }

    pub fn push(&self, frame: &[u8]) -> Result<()> {
        let mut buffer = gstreamer::Buffer::with_size(self.video_info.size())
            .expect("failed to create a new gstreamer buffer");
        {
            let buffer = buffer.get_mut().unwrap();
            let mut video_frame =
                VideoFrameRef::from_buffer_ref_writable(buffer, &self.video_info).unwrap();
            let plane_data = video_frame.plane_data_mut(0).unwrap();
            unsafe {
                ptr::copy_nonoverlapping(frame.as_ptr(), plane_data.as_mut_ptr(), frame.len());
            }
        }
        self.appsrc.push_buffer(buffer)?;
        Ok(())
    }
}

impl Drop for Downstream {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}
