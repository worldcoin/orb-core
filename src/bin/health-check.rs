#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

use clap::Parser;
use eyre::{bail, ensure, Context, Result};
use futures::{
    future::{select, Either},
    pin_mut,
};
use orb::{
    async_main,
    backend::init_cert,
    brokers::Orb,
    cli::Cli,
    config::Config,
    consts::IR_CAMERA_FRAME_RATE,
    led::{self, Engine},
    logger,
    mcu::{self, Mcu},
    monitor::{self, cpu::Monitor as _},
    plans::{health_check, MasterPlan},
    sound::{self, Melody, Player, Type},
};

use std::{sync::Arc, time::Duration};
use tokio::{signal::ctrl_c, sync::Mutex};

fn main() -> Result<()> {
    async_main(run(Cli::parse()))
}

async fn run(_cli: Cli) -> Result<()> {
    logger::init::<false>();
    init_cert().wrap_err("initializing root certificate")?;
    ensure!(sodiumoxide::init().is_ok(), "sodiumoxide initialization failure");

    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let mut sound = sound::Jetson::spawn(Arc::clone(&config), true, cpu_monitor.clone()).await?;
    sound.build(Type::Melody(Melody::BootUp))?.volume(0.5).push()?;
    let main_mcu = mcu::main::Jetson::spawn()?;
    let net_monitor = monitor::net::Jetson::spawn(Arc::clone(&config))?;
    let led = led::Jetson::spawn(main_mcu.clone());
    led.pause();
    let mut orb = Box::pin(
        Orb::builder()
            .config(config)
            .sound(Box::new(sound))
            .led(Box::new(led))
            .main_mcu(Box::new(main_mcu))
            .net_monitor(Box::new(net_monitor))
            .cpu_monitor(cpu_monitor)
            .build(),
    )
    .await?;

    // reset optics state to a known state (also allows us to correctly initialize the `struct Orb` state)
    orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
    orb.disable_ir_led().await?;
    orb.main_mcu.send(mcu::main::Input::LiquidLens(None)).await?;

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
