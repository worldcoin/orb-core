//! Egui app.

use crate::{
    downstream::Downstream, upstream::Upstream, LIVESTREAM_FRAME_HEIGHT, LIVESTREAM_FRAME_WIDTH,
};
use eframe::{
    wgpu::{
        Extent3d, FilterMode, ImageDataLayout, Queue, Texture, TextureDescriptor, TextureDimension,
        TextureFormat, TextureUsages, TextureViewDescriptor,
    },
    CreationContext, Frame,
};
use egui::{load::SizedTexture, CentralPanel, Context, Image, RawInput, Rect, TextureId};
use livestream_event::{Event, Pos2};
use std::{
    mem::take,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const EVENTS_INTERVAL: Duration = Duration::from_millis(50); // 20 FPS

#[allow(clippy::cast_precision_loss)]
const TEXTURE_SIZE: egui::Vec2 =
    egui::Vec2::new(LIVESTREAM_FRAME_WIDTH as f32, LIVESTREAM_FRAME_HEIGHT as f32);

const TEXTURE_EXTENT: Extent3d = Extent3d {
    width: LIVESTREAM_FRAME_WIDTH,
    height: LIVESTREAM_FRAME_HEIGHT,
    depth_or_array_layers: 1,
};

const DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(LIVESTREAM_FRAME_WIDTH * 4),
    rows_per_image: Some(LIVESTREAM_FRAME_HEIGHT),
};

/// Egui app.
pub struct App {
    texture_id: TextureId,
    texture_rect: Rect,
    upstream: Upstream,
    events_buffer: Vec<Event>,
    events_last_sent: Instant,
    join_handle: Option<JoinHandle<()>>,
}

impl App {
    /// Creates a new [`App`].
    #[must_use]
    pub fn new(cc: &CreationContext<'_>, downstream: Downstream, upstream: Upstream) -> Self {
        let wgpu = cc.wgpu_render_state.as_ref().expect("renderer is not wgpu");
        let queue = Arc::clone(&wgpu.queue);
        let texture = wgpu.device.create_texture(&TextureDescriptor {
            label: None,
            size: TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_id = wgpu.renderer.write().register_native_texture(
            &wgpu.device,
            &texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let join_handle =
            thread::spawn(move || downstream_update_loop(&downstream, &queue, &texture));
        Self {
            texture_id,
            texture_rect: Rect::from_min_max(egui::Pos2::ZERO, TEXTURE_SIZE.to_pos2()),
            upstream,
            events_buffer: Vec::new(),
            events_last_sent: Instant::now(),
            join_handle: Some(join_handle),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        CentralPanel::default().show(ctx, |ui| {
            let width_ratio = ui.available_width() / TEXTURE_SIZE.x;
            let height_ratio = ui.available_height() / TEXTURE_SIZE.y;
            let texture_size = if width_ratio < 1.0 && width_ratio < height_ratio {
                TEXTURE_SIZE * width_ratio
            } else if height_ratio < 1.0 {
                TEXTURE_SIZE * height_ratio
            } else {
                TEXTURE_SIZE
            };
            self.texture_rect =
                Rect::from_center_size((ui.available_size() / 2.0).to_pos2(), texture_size);
            ui.put(
                self.texture_rect,
                Image::from_texture(SizedTexture::new(self.texture_id, texture_size)),
            );
        });
        if self.join_handle.as_ref().map_or(true, JoinHandle::is_finished) {
            self.join_handle
                .take()
                .expect("downstream update loop finished")
                .join()
                .expect("downstream update loop panicked");
        }
        ctx.request_repaint();
    }

    #[allow(clippy::cast_precision_loss)]
    fn raw_input_hook(&mut self, _ctx: &Context, raw_input: &mut RawInput) {
        for event in &raw_input.events {
            let Ok(mut event) = event.try_into() else { continue };
            if let Event::PointerMoved(pos) | Event::PointerButton { pos, .. } = &mut event {
                *pos = Pos2 {
                    x: ((pos.x - self.texture_rect.min.x) / self.texture_rect.width())
                        .clamp(0.0, 1.0)
                        * TEXTURE_EXTENT.width as f32,
                    y: ((pos.y - self.texture_rect.min.y) / self.texture_rect.height())
                        .clamp(0.0, 1.0)
                        * TEXTURE_EXTENT.height as f32,
                };
            }
            self.events_buffer.push(event);
        }
        let now = Instant::now();
        if now.duration_since(self.events_last_sent) > EVENTS_INTERVAL {
            let bytes = rkyv::to_bytes::<_, 4096>(&take(&mut self.events_buffer))
                .expect("failed to serialize egui input");
            self.upstream.send(&bytes).expect("failed to write to upstream");
            self.events_last_sent = now;
        }
    }
}

fn downstream_update_loop(downstream: &Downstream, queue: &Arc<Queue>, texture: &Texture) -> ! {
    downstream.start().unwrap();
    loop {
        let data = downstream
            .pull_sample()
            .expect("failed to pull sample")
            .buffer_owned()
            .expect("unable to obtain sample buffer")
            .into_mapped_buffer_readable()
            .expect("unable to obtain readable mapped buffer");
        queue.write_texture(texture.as_image_copy(), &data, DATA_LAYOUT, TEXTURE_EXTENT);
        queue.submit(None);
    }
}
