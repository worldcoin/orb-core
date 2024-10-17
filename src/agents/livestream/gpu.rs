use super::app::{App, Expanded};
use crate::consts::{
    DEPTH_HEIGHT, DEPTH_WIDTH, IR_HEIGHT, IR_WIDTH, LIVESTREAM_FRAME_HEIGHT,
    LIVESTREAM_FRAME_WIDTH, RGB_DEFAULT_HEIGHT, RGB_DEFAULT_WIDTH, RGB_NATIVE_HEIGHT,
    RGB_NATIVE_WIDTH, RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH, THERMAL_HEIGHT, THERMAL_WIDTH,
};
use egui::{
    load::SizedTexture, CentralPanel, Context, Event, FullOutput, Image, Pos2, RawInput, Rect,
    TextureId, Vec2,
};
use egui_wgpu::{
    wgpu::{
        Buffer, BufferDescriptor, BufferUsages, BufferView, CommandEncoder,
        CommandEncoderDescriptor, Device, DeviceDescriptor, Extent3d, Features, FilterMode,
        ImageCopyBuffer, ImageDataLayout, Instance, Limits, LoadOp, Maintain, MapMode, Operations,
        Queue, RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, StoreOp,
        Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        TextureViewDescriptor, COPY_BYTES_PER_ROW_ALIGNMENT,
    },
    Renderer, ScreenDescriptor,
};
use eyre::{Error, Result};
use orb_royale::DepthPoint;
use std::{mem::size_of, sync::Arc, time::Instant};

#[allow(clippy::cast_precision_loss)]
const RGB_ASPECT_RATIO: f32 = RGB_NATIVE_WIDTH as f32 / RGB_NATIVE_HEIGHT as f32;

#[allow(clippy::cast_precision_loss)]
const IR_EYE_ASPECT_RATIO: f32 = IR_WIDTH as f32 / IR_HEIGHT as f32;

#[allow(clippy::cast_precision_loss)]
const IR_FACE_ASPECT_RATIO: f32 = IR_HEIGHT as f32 / IR_WIDTH as f32;

#[allow(clippy::cast_precision_loss)]
const THERMAL_ASPECT_RATIO: f32 = THERMAL_WIDTH as f32 / THERMAL_HEIGHT as f32;

#[allow(clippy::cast_precision_loss)]
const DEPTH_ASPECT_RATIO: f32 = DEPTH_WIDTH as f32 / DEPTH_HEIGHT as f32;

const PIXELS_PER_POINT: f32 = 1.0;

const SCREEN_DESCRIPTOR: ScreenDescriptor = ScreenDescriptor {
    size_in_pixels: [LIVESTREAM_FRAME_WIDTH, LIVESTREAM_FRAME_HEIGHT],
    pixels_per_point: PIXELS_PER_POINT,
};

#[allow(clippy::cast_precision_loss)]
const SCREEN_RECT: Rect = Rect::from_min_max(
    Pos2::new(0.0, 0.0),
    Pos2::new(LIVESTREAM_FRAME_WIDTH as f32, LIVESTREAM_FRAME_HEIGHT as f32),
);

const OUTPUT_TEXTURE_EXTENT: Extent3d = Extent3d {
    width: LIVESTREAM_FRAME_WIDTH,
    height: LIVESTREAM_FRAME_HEIGHT,
    depth_or_array_layers: 1,
};

#[allow(clippy::cast_possible_truncation)]
const OUTPUT_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(PADDED_BYTES_PER_ROW as u32),
    rows_per_image: None,
};

const CAMERA_IR_EYE_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: IR_WIDTH, height: IR_HEIGHT, depth_or_array_layers: 1 };

const CAMERA_IR_EYE_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(IR_WIDTH * 4),
    rows_per_image: Some(IR_HEIGHT),
};

const CAMERA_IR_FACE_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: IR_HEIGHT, height: IR_WIDTH, depth_or_array_layers: 1 };

const CAMERA_IR_FACE_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(IR_HEIGHT * 4),
    rows_per_image: Some(IR_WIDTH),
};

const CAMERA_RGB_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: RGB_DEFAULT_WIDTH, height: RGB_DEFAULT_HEIGHT, depth_or_array_layers: 1 };

const CAMERA_RGB_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(RGB_DEFAULT_WIDTH * 4),
    rows_per_image: Some(RGB_DEFAULT_HEIGHT),
};

const CAMERA_RGB_REDUCED_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: RGB_REDUCED_WIDTH, height: RGB_REDUCED_HEIGHT, depth_or_array_layers: 1 };

const CAMERA_RGB_REDUCED_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(RGB_REDUCED_WIDTH * 4),
    rows_per_image: Some(RGB_REDUCED_HEIGHT),
};

const CAMERA_THERMAL_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: THERMAL_WIDTH, height: THERMAL_HEIGHT, depth_or_array_layers: 1 };

const CAMERA_THERMAL_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(THERMAL_WIDTH * 4),
    rows_per_image: Some(THERMAL_HEIGHT),
};

const CAMERA_DEPTH_TEXTURE_EXTENT: Extent3d =
    Extent3d { width: DEPTH_WIDTH, height: DEPTH_HEIGHT, depth_or_array_layers: 1 };

const CAMERA_DEPTH_DATA_LAYOUT: ImageDataLayout = ImageDataLayout {
    offset: 0,
    bytes_per_row: Some(DEPTH_WIDTH * 4),
    rows_per_image: Some(DEPTH_HEIGHT),
};

const PADDED_BYTES_PER_ROW: usize = {
    let unpadded_bytes_per_row = LIVESTREAM_FRAME_WIDTH as usize * size_of::<u32>();
    let align = COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded_bytes_per_row_padding = (align - unpadded_bytes_per_row % align) % align;
    unpadded_bytes_per_row + padded_bytes_per_row_padding
};

pub struct Gpu {
    device: Device,
    queue: Queue,
    output_buffer: Arc<Buffer>,
    output_texture: Texture,
    camera_ir_eye_texture_id: TextureId,
    camera_ir_eye_texture: Texture,
    camera_ir_eye_frame: Box<[u8]>,
    camera_ir_face_texture_id: TextureId,
    camera_ir_face_texture: Texture,
    camera_ir_face_frame: Box<[u8]>,
    camera_rgb_texture_id: TextureId,
    camera_rgb_texture: Texture,
    camera_rgb_frame: Box<[u8]>,
    camera_rgb_reduced_texture_id: TextureId,
    camera_rgb_reduced_texture: Texture,
    camera_rgb_reduced_frame: Box<[u8]>,
    camera_rgb_latest_reduced: bool,
    camera_thermal_texture_id: TextureId,
    camera_thermal_texture: Texture,
    camera_thermal_frame: Box<[u8]>,
    camera_depth_texture_id: TextureId,
    camera_depth_texture: Texture,
    camera_depth_frame: Box<[u8]>,
    egui: Context,
    egui_rend: Renderer,
    start_time: Instant,
    last_time: Instant,
    pub app: App,
}

impl Gpu {
    #[allow(clippy::too_many_lines)]
    pub async fn new() -> Result<Self> {
        let adapter = Instance::default()
            .request_adapter(&RequestAdapterOptions::default())
            .await
            .expect("Failed to find an appropriate adapter");
        tracing::info!("Selected adapter: {:?}", adapter.get_info());

        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label: None,
                    required_features: Features::empty(),
                    required_limits: Limits::downlevel_defaults(),
                },
                None,
            )
            .await?;

        let output_buffer = Arc::new(device.create_buffer(&BufferDescriptor {
            label: None,
            size: (PADDED_BYTES_PER_ROW * LIVESTREAM_FRAME_HEIGHT as usize) as u64,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let output_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: OUTPUT_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let camera_ir_eye_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_IR_EYE_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_ir_eye_frame =
            vec![0; IR_WIDTH as usize * IR_HEIGHT as usize * 4].into_boxed_slice();

        let camera_ir_face_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_IR_FACE_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_ir_face_frame =
            vec![0; IR_WIDTH as usize * IR_HEIGHT as usize * 4].into_boxed_slice();

        let camera_rgb_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_RGB_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_rgb_frame =
            vec![0; RGB_DEFAULT_WIDTH as usize * RGB_DEFAULT_HEIGHT as usize * 4]
                .into_boxed_slice();

        let camera_rgb_reduced_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_RGB_REDUCED_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_rgb_reduced_frame =
            vec![0; RGB_REDUCED_WIDTH as usize * RGB_REDUCED_HEIGHT as usize * 4]
                .into_boxed_slice();

        let camera_thermal_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_THERMAL_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_thermal_frame =
            vec![0; THERMAL_WIDTH as usize * THERMAL_HEIGHT as usize * 4].into_boxed_slice();

        let camera_depth_texture = device.create_texture(&TextureDescriptor {
            label: None,
            size: CAMERA_DEPTH_TEXTURE_EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let camera_depth_frame =
            vec![0; DEPTH_WIDTH as usize * DEPTH_HEIGHT as usize * 4].into_boxed_slice();

        let egui = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        egui.set_fonts(fonts);
        egui.begin_frame(RawInput { screen_rect: Some(SCREEN_RECT), ..Default::default() });
        let mut egui_rend = Renderer::new(&device, TextureFormat::Rgba8UnormSrgb, None, 1);
        let camera_ir_eye_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_ir_eye_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let camera_ir_face_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_ir_face_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let camera_rgb_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_rgb_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let camera_rgb_reduced_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_rgb_reduced_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let camera_thermal_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_thermal_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );
        let camera_depth_texture_id = egui_rend.register_native_texture(
            &device,
            &camera_depth_texture.create_view(&TextureViewDescriptor::default()),
            FilterMode::Linear,
        );

        Ok(Self {
            device,
            queue,
            output_buffer,
            output_texture,
            camera_ir_eye_texture_id,
            camera_ir_eye_texture,
            camera_ir_eye_frame,
            camera_ir_face_texture_id,
            camera_ir_face_texture,
            camera_ir_face_frame,
            camera_rgb_texture_id,
            camera_rgb_texture,
            camera_rgb_frame,
            camera_rgb_reduced_texture_id,
            camera_rgb_reduced_texture,
            camera_rgb_reduced_frame,
            camera_rgb_latest_reduced: false,
            camera_thermal_texture_id,
            camera_thermal_texture,
            camera_thermal_frame,
            camera_depth_texture_id,
            camera_depth_texture,
            camera_depth_frame,
            egui,
            egui_rend,
            start_time: Instant::now(),
            last_time: Instant::now(),
            app: App::default(),
        })
    }

    pub fn clear_textures(&mut self) {
        self.camera_ir_eye_frame.fill(0);
        self.camera_ir_face_frame.fill(0);
        self.camera_rgb_frame.fill(0);
        self.camera_rgb_reduced_frame.fill(0);
        self.camera_rgb_latest_reduced = false;
        self.camera_thermal_frame.fill(0);
        self.camera_depth_frame.fill(0);
        self.queue.write_texture(
            self.camera_ir_eye_texture.as_image_copy(),
            self.camera_ir_eye_frame.as_ref(),
            CAMERA_IR_EYE_DATA_LAYOUT,
            CAMERA_IR_EYE_TEXTURE_EXTENT,
        );
        self.queue.write_texture(
            self.camera_ir_face_texture.as_image_copy(),
            self.camera_ir_face_frame.as_ref(),
            CAMERA_IR_FACE_DATA_LAYOUT,
            CAMERA_IR_FACE_TEXTURE_EXTENT,
        );
        self.queue.write_texture(
            self.camera_rgb_texture.as_image_copy(),
            self.camera_rgb_frame.as_ref(),
            CAMERA_RGB_DATA_LAYOUT,
            CAMERA_RGB_TEXTURE_EXTENT,
        );
        self.queue.write_texture(
            self.camera_rgb_reduced_texture.as_image_copy(),
            self.camera_rgb_reduced_frame.as_ref(),
            CAMERA_RGB_REDUCED_DATA_LAYOUT,
            CAMERA_RGB_REDUCED_TEXTURE_EXTENT,
        );
        self.queue.write_texture(
            self.camera_thermal_texture.as_image_copy(),
            self.camera_thermal_frame.as_ref(),
            CAMERA_THERMAL_DATA_LAYOUT,
            CAMERA_THERMAL_TEXTURE_EXTENT,
        );
        self.queue.write_texture(
            self.camera_depth_texture.as_image_copy(),
            self.camera_depth_frame.as_ref(),
            CAMERA_DEPTH_DATA_LAYOUT,
            CAMERA_DEPTH_TEXTURE_EXTENT,
        );
    }

    pub fn update_camera_ir_eye(&mut self, frame: &[u8]) {
        update_camera_ir_frame(self.camera_ir_eye_frame.as_mut_ptr(), frame);
        self.queue.write_texture(
            self.camera_ir_eye_texture.as_image_copy(),
            self.camera_ir_eye_frame.as_ref(),
            CAMERA_IR_EYE_DATA_LAYOUT,
            CAMERA_IR_EYE_TEXTURE_EXTENT,
        );
    }

    pub fn update_camera_ir_face(&mut self, frame: &[u8]) {
        update_camera_ir_frame(self.camera_ir_face_frame.as_mut_ptr(), frame);
        self.queue.write_texture(
            self.camera_ir_face_texture.as_image_copy(),
            self.camera_ir_face_frame.as_ref(),
            CAMERA_IR_FACE_DATA_LAYOUT,
            CAMERA_IR_FACE_TEXTURE_EXTENT,
        );
    }

    pub fn update_camera_rgb(&mut self, frame: &[u8], width: u32, height: u32) {
        if width == RGB_REDUCED_WIDTH && height == RGB_REDUCED_HEIGHT {
            update_camera_rgb_frame(
                self.camera_rgb_reduced_frame.as_mut_ptr(),
                frame,
                RGB_REDUCED_WIDTH as usize,
                RGB_REDUCED_HEIGHT as usize,
                1,
            );
            self.queue.write_texture(
                self.camera_rgb_reduced_texture.as_image_copy(),
                self.camera_rgb_reduced_frame.as_ref(),
                CAMERA_RGB_REDUCED_DATA_LAYOUT,
                CAMERA_RGB_REDUCED_TEXTURE_EXTENT,
            );
            self.camera_rgb_latest_reduced = true;
        } else {
            if width == RGB_DEFAULT_WIDTH && height == RGB_DEFAULT_HEIGHT {
                update_camera_rgb_frame(
                    self.camera_rgb_frame.as_mut_ptr(),
                    frame,
                    RGB_DEFAULT_WIDTH as usize,
                    RGB_DEFAULT_HEIGHT as usize,
                    1,
                );
            } else if width == RGB_NATIVE_WIDTH && height == RGB_NATIVE_HEIGHT {
                update_camera_rgb_frame(
                    self.camera_rgb_frame.as_mut_ptr(),
                    frame,
                    RGB_DEFAULT_WIDTH as usize,
                    RGB_DEFAULT_HEIGHT as usize,
                    2,
                );
            }
            self.queue.write_texture(
                self.camera_rgb_texture.as_image_copy(),
                self.camera_rgb_frame.as_ref(),
                CAMERA_RGB_DATA_LAYOUT,
                CAMERA_RGB_TEXTURE_EXTENT,
            );
            self.camera_rgb_latest_reduced = false;
        }
    }

    pub fn update_camera_thermal(&mut self, frame: &[u8]) {
        update_camera_thermal_frame(self.camera_thermal_frame.as_mut_ptr(), frame);
        self.queue.write_texture(
            self.camera_thermal_texture.as_image_copy(),
            self.camera_thermal_frame.as_ref(),
            CAMERA_THERMAL_DATA_LAYOUT,
            CAMERA_THERMAL_TEXTURE_EXTENT,
        );
    }

    pub fn update_camera_depth(&mut self, frame: &[DepthPoint]) {
        update_camera_depth_frame(self.camera_depth_frame.as_mut_ptr(), frame);
        self.queue.write_texture(
            self.camera_depth_texture.as_image_copy(),
            self.camera_depth_frame.as_ref(),
            CAMERA_DEPTH_DATA_LAYOUT,
            CAMERA_DEPTH_TEXTURE_EXTENT,
        );
    }

    pub fn render(
        &mut self,
        events: Vec<Event>,
        f: impl FnOnce(BufferView) -> Result<()> + Send + 'static,
    ) {
        let now = Instant::now();
        self.egui.begin_frame(RawInput {
            time: Some(now.duration_since(self.start_time).as_secs_f64()),
            predicted_dt: now.duration_since(self.last_time).as_secs_f32(),
            events,
            ..Default::default()
        });
        self.last_time = now;
        let mut encoder =
            self.device.create_command_encoder(&CommandEncoderDescriptor { label: None });
        self.render_egui(&mut encoder);
        encoder.copy_texture_to_buffer(
            self.output_texture.as_image_copy(),
            ImageCopyBuffer { buffer: &self.output_buffer, layout: OUTPUT_DATA_LAYOUT },
            OUTPUT_TEXTURE_EXTENT,
        );

        let submission_index = self.queue.submit(Some(encoder.finish()));
        let output_buffer = Arc::clone(&self.output_buffer);
        self.output_buffer.slice(..).map_async(MapMode::Read, move |result| {
            let runner = || {
                result?;
                f(output_buffer.slice(..).get_mapped_range())?;
                output_buffer.unmap();
                Ok::<(), Error>(())
            };
            if let Err(err) = runner() {
                tracing::error!("Livestream render error: {err:?}");
            }
        });
        self.device.poll(Maintain::WaitForSubmissionIndex(submission_index));
    }

    fn render_egui(&mut self, encoder: &mut CommandEncoder) {
        self.update_egui();
        let FullOutput { textures_delta, shapes, pixels_per_point, .. } = self.egui.end_frame();
        let clipped_primitives = self.egui.tessellate(shapes, pixels_per_point);
        for (id, image_delta) in textures_delta.set {
            self.egui_rend.update_texture(&self.device, &self.queue, id, &image_delta);
        }
        self.egui_rend.update_buffers(
            &self.device,
            &self.queue,
            encoder,
            &clipped_primitives,
            &SCREEN_DESCRIPTOR,
        );
        let color_attachment = self.output_texture.create_view(&TextureViewDescriptor::default());
        {
            let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("egui_render"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &color_attachment,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.egui_rend.render(&mut render_pass, &clipped_primitives, &SCREEN_DESCRIPTOR);
        }
        for id in textures_delta.free {
            self.egui_rend.free_texture(&id);
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    fn update_egui(&mut self) {
        macro_rules! expanded_viewport {
            ($ui:expr, $texture_id:expr, $aspect_ratio:expr, $update_fn:ident) => {
                let rect = Rect::from_center_size(
                    Pos2::new(
                        LIVESTREAM_FRAME_WIDTH as f32 / 2.0,
                        LIVESTREAM_FRAME_HEIGHT as f32 / 2.0,
                    ),
                    Vec2::new(
                        LIVESTREAM_FRAME_HEIGHT as f32 * $aspect_ratio,
                        LIVESTREAM_FRAME_HEIGHT as f32,
                    ),
                );
                let response =
                    $ui.put(rect, Image::from_texture(SizedTexture::new($texture_id, rect.size())));
                self.app.$update_fn($ui, rect, &response);
            };
        }

        let camera_rgb_texture_id = self.camera_rgb_texture_id();
        CentralPanel::default().show(&self.egui, |ui| match self.app.expanded() {
            Expanded::None => {
                let rgb_rem_x = LIVESTREAM_FRAME_WIDTH as f32
                    - LIVESTREAM_FRAME_HEIGHT as f32 * RGB_ASPECT_RATIO;
                let rgb_rect = Rect::from_min_max(
                    Pos2::new(rgb_rem_x, 0.0),
                    Pos2::new(LIVESTREAM_FRAME_WIDTH as f32, LIVESTREAM_FRAME_HEIGHT as f32),
                );
                let ir_eye_rem_y = LIVESTREAM_FRAME_HEIGHT as f32 - rgb_rem_x / IR_EYE_ASPECT_RATIO;
                let ir_eye_rect = Rect::from_min_max(
                    Pos2::new(0.0, ir_eye_rem_y),
                    Pos2::new(rgb_rem_x, LIVESTREAM_FRAME_HEIGHT as f32),
                );
                let ir_face_rem_x = rgb_rem_x - ir_eye_rem_y * IR_FACE_ASPECT_RATIO;
                let ir_face_rect = Rect::from_min_max(
                    Pos2::new(ir_face_rem_x, 0.0),
                    Pos2::new(rgb_rem_x, ir_eye_rem_y),
                );
                let thermal_rem_x = ir_face_rem_x - ir_eye_rem_y * THERMAL_ASPECT_RATIO;
                let thermal_rect = Rect::from_min_max(
                    Pos2::new(thermal_rem_x, 0.0),
                    Pos2::new(ir_face_rem_x, ir_eye_rem_y),
                );
                let depth_rem_x = thermal_rem_x - ir_eye_rem_y * DEPTH_ASPECT_RATIO;
                let depth_rect = Rect::from_min_max(
                    Pos2::new(depth_rem_x, 0.0),
                    Pos2::new(thermal_rem_x, ir_eye_rem_y),
                );
                let dashboard_rect =
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(depth_rem_x, ir_eye_rem_y));
                let rgb_response = ui.put(
                    rgb_rect,
                    Image::from_texture(SizedTexture::new(camera_rgb_texture_id, rgb_rect.size())),
                );
                let ir_eye_response = ui.put(
                    ir_eye_rect,
                    Image::from_texture(SizedTexture::new(
                        self.camera_ir_eye_texture_id,
                        ir_eye_rect.size(),
                    )),
                );
                let ir_face_response = ui.put(
                    ir_face_rect,
                    Image::from_texture(SizedTexture::new(
                        self.camera_ir_face_texture_id,
                        ir_face_rect.size(),
                    )),
                );
                let thermal_response = ui.put(
                    thermal_rect,
                    Image::from_texture(SizedTexture::new(
                        self.camera_thermal_texture_id,
                        thermal_rect.size(),
                    )),
                );
                let depth_response = ui.put(
                    depth_rect,
                    Image::from_texture(SizedTexture::new(
                        self.camera_depth_texture_id,
                        depth_rect.size(),
                    )),
                );
                self.app.update_rgb_viewport(ui, rgb_rect, &rgb_response);
                self.app.update_ir_eye_viewport(ui, ir_eye_rect, &ir_eye_response);
                self.app.update_ir_face_viewport(ui, ir_face_rect, &ir_face_response);
                self.app.update_thermal_viewport(ui, thermal_rect, &thermal_response);
                self.app.update_depth_viewport(ui, depth_rect, &depth_response);
                self.app.update_dashboard(ui, dashboard_rect);
            }
            Expanded::Rgb => {
                expanded_viewport!(
                    ui,
                    camera_rgb_texture_id,
                    RGB_ASPECT_RATIO,
                    update_rgb_viewport
                );
            }
            Expanded::IrEye => {
                expanded_viewport!(
                    ui,
                    self.camera_ir_eye_texture_id,
                    IR_EYE_ASPECT_RATIO,
                    update_ir_eye_viewport
                );
            }
            Expanded::IrFace => {
                expanded_viewport!(
                    ui,
                    self.camera_ir_face_texture_id,
                    IR_FACE_ASPECT_RATIO,
                    update_ir_face_viewport
                );
            }
            Expanded::Thermal => {
                expanded_viewport!(
                    ui,
                    self.camera_thermal_texture_id,
                    THERMAL_ASPECT_RATIO,
                    update_thermal_viewport
                );
            }
            Expanded::Depth => {
                expanded_viewport!(
                    ui,
                    self.camera_depth_texture_id,
                    DEPTH_ASPECT_RATIO,
                    update_depth_viewport
                );
            }
        });
        self.app.update_context(&self.egui);
    }

    fn camera_rgb_texture_id(&self) -> TextureId {
        if self.camera_rgb_latest_reduced {
            self.camera_rgb_reduced_texture_id
        } else {
            self.camera_rgb_texture_id
        }
    }
}

#[allow(clippy::cast_ptr_alignment)]
fn update_camera_rgb_frame(
    buffer: *mut u8,
    frame: &[u8],
    width: usize,
    height: usize,
    divider: usize,
) {
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..height {
            let row_in = frame.as_ptr().add(y * width * 3 * divider * divider);
            let mut px_out = buffer.cast::<u32>().add(y * width);
            for x in 0..width {
                let px_in = row_in.add(x * 3 * divider);
                let b = *px_in;
                let g = *px_in.add(1);
                let r = *px_in.add(2);
                *px_out = u32::from_le_bytes([r, g, b, 0xFF]);
                px_out = px_out.add(1);
            }
        }
    }
}

#[allow(clippy::cast_ptr_alignment)]
fn update_camera_ir_frame(buffer: *mut u8, frame: &[u8]) {
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..IR_HEIGHT as usize {
            let row_in = frame.as_ptr().add(y * IR_WIDTH as usize);
            let mut px_out = buffer.cast::<u32>().add(y * IR_WIDTH as usize);
            for x in 0..IR_WIDTH as usize {
                let px_in = row_in.add(x);
                let px = *px_in;
                *px_out = u32::from_le_bytes([px, px, px, 0xFF]);
                px_out = px_out.add(1);
            }
        }
    }
}

#[allow(clippy::cast_ptr_alignment)]
fn update_camera_thermal_frame(buffer: *mut u8, frame: &[u8]) {
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..THERMAL_HEIGHT as usize {
            let row_in = frame.as_ptr().add(y * THERMAL_WIDTH as usize);
            let mut px_out = buffer.cast::<u32>().add(y * THERMAL_WIDTH as usize);
            for x in 0..THERMAL_WIDTH as usize {
                let px_in = row_in.add(x);
                let px = *px_in;
                *px_out = u32::from_le_bytes([px, px, px, 0xFF]);
                px_out = px_out.add(1);
            }
        }
    }
}

#[allow(clippy::cast_ptr_alignment, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn update_camera_depth_frame(buffer: *mut u8, frame: &[DepthPoint]) {
    // This loop was optimized for vectorization.
    unsafe {
        for y in 0..DEPTH_HEIGHT as usize {
            let row_in = frame.as_ptr().add(y * DEPTH_WIDTH as usize);
            let mut px_out = buffer.cast::<u32>().add(y * DEPTH_WIDTH as usize);
            for x in 0..DEPTH_WIDTH as usize {
                let px_in = row_in.add(x);
                let [r, g, b] = (*px_in).to_rgb();
                *px_out = u32::from_be_bytes([0, r, g, b]);
                px_out = px_out.add(1);
            }
        }
    }
}
