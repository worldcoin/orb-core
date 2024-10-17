//! Orb Livestream client.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc, clippy::missing_panics_doc)]

pub mod app;
pub mod cli;
pub mod downstream;
pub mod upstream;

use app::App;
use clap::Parser as _;
use cli::Cli;
use downstream::Downstream;
use egui::ViewportBuilder;
use eyre::{eyre, Result};
use upstream::Upstream;

/// Livestream frame width.
pub const LIVESTREAM_FRAME_WIDTH: u32 = 1920;

/// Livestream frame height.
pub const LIVESTREAM_FRAME_HEIGHT: u32 = 1080;

fn main() -> Result<()> {
    color_eyre::install()?;
    gstreamer::init()?;
    let Cli { ip } = Cli::parse();

    let upstream = Upstream::new(ip)?;
    let downstream = Downstream::new()?;

    #[allow(clippy::cast_precision_loss)]
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: ViewportBuilder::default()
            .with_inner_size([LIVESTREAM_FRAME_WIDTH as f32, LIVESTREAM_FRAME_HEIGHT as f32])
            .with_max_inner_size([LIVESTREAM_FRAME_WIDTH as f32, LIVESTREAM_FRAME_HEIGHT as f32]),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "Orb Livestream Client",
        options,
        Box::new(|cc| Box::new(App::new(cc, downstream, upstream))),
    )
    .map_err(|err| eyre!("failed to run eframe app: {err}"))?;

    Ok(())
}
