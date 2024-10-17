#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![cfg_attr(not(feature = "livestream"), allow(unused_variables))]

use clap::Parser;
use eyre::{bail, ensure, Result};
use futures::{
    future::{select, Either},
    pin_mut,
};
#[cfg(feature = "livestream")]
use orb::consts::RGB_FPS;
use orb::{
    async_main,
    brokers::Orb,
    cli::Cli,
    config::Config,
    consts::IR_CAMERA_FRAME_RATE,
    logger, mcu, monitor,
    plans::{health_check, MasterPlan},
    ui::{self, Engine},
};

use std::{sync::Arc, time::Duration};
use tokio::{signal::ctrl_c, sync::Mutex};

fn main() -> Result<()> {
    async_main(run(Cli::parse()))
}

async fn run(cli: Cli) -> Result<()> {
    logger::init::<false>();
    ensure!(sodiumoxide::init().is_ok(), "sodiumoxide initialization failure");

    let ui = ui::Jetson::spawn();
    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    config.lock().await.propagate_to_ui(&ui);
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let main_mcu = mcu::main::Jetson::spawn()?;
    let net_monitor = monitor::net::Jetson::spawn(Arc::clone(&config))?;
    ui.pause();
    let mut builder = Orb::builder();
    builder = builder
        .config(config)
        .ui(Box::new(ui))
        .main_mcu(Box::new(main_mcu))
        .net_monitor(Box::new(net_monitor))
        .cpu_monitor(cpu_monitor);
    let mut orb = Box::pin(builder.build()).await?;

    // reset optics state to a known state (also allows us to correctly initialize the `struct Orb` state)
    orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
    orb.disable_ir_led().await?;
    orb.main_mcu.send(mcu::main::Input::LiquidLens(None)).await?;

    #[cfg(feature = "livestream")]
    if cli.livestream {
        orb.start_rgb_camera(RGB_FPS).await?;
        orb.enable_livestream()?;
    }

    let mut health_check = health_check::Plan::default();
    let result = {
        let health_check = health_check.run(&mut orb);
        let ctrl_c = ctrl_c();
        pin_mut!(health_check);
        pin_mut!(ctrl_c);
        match select(health_check, ctrl_c).await {
            Either::Left((Err(err), _)) => {
                tracing::error!("Broker exited with error: {}", err);
                Err(err)
            }
            Either::Left((result, _)) => result,
            Either::Right((result, _)) => result.map(|()| false).map_err(Into::into),
        }
    };
    MasterPlan::builder()
        .build()
        .await?
        .reset_hardware(&mut orb, Duration::from_millis(100))
        .await?;
    if result? {
        tracing::info!("All checks passed!");
    } else {
        bail!("Health check failure!");
    }
    Ok(())
}
