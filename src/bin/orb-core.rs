#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

use clap::Parser;
use eyre::{ensure, Result, WrapErr};
use futures::{pin_mut, prelude::*, select};

use orb::{
    async_main,
    backend::init_cert,
    brokers::{DefaultObserverPlan, Observer, Orb},
    cli::Cli,
    config::Config,
    led::{self, Engine},
    logger,
    logger::{DATADOG, NO_TAGS},
    mcu::{self, Mcu},
    monitor::{self, cpu::Monitor as _, net::Monitor as _},
    plans::{warmup, MasterPlan},
    sound::{self, Melody, Player},
};
use std::{
    convert::TryInto,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{signal::ctrl_c, sync::Mutex};

fn main() -> Result<()> {
    async_main(run(Cli::parse()))
}

#[allow(let_underscore_drop, clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<()> {
    logger::init::<false>();
    init_cert().wrap_err("initializing root certificate")?;
    ensure!(sodiumoxide::init().is_ok(), "sodiumoxide initialization failure");

    DATADOG.incr("orb.main.count.global.starting_main_program", NO_TAGS)?;
    let t = SystemTime::now();

    // When the orb boots up for the first time, there is no internet
    // connection, so we must rely solely on the local configuration. In any
    // case, this configuration setup is only used for playing basic startup
    // sounds.
    let config = Arc::new(Mutex::new(Config::load_or_default().await));

    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let mut sound = Box::new(
        sound::Jetson::spawn(Arc::clone(&config), cli.ignore_missing_sounds, cpu_monitor.clone())
            .await?,
    );

    let main_mcu = Box::new(mcu::main::Jetson::spawn()?);
    let led = led::Jetson::spawn(main_mcu.clone());
    let (net_monitor, net_monitor_trigger) =
        monitor::net::Jetson::spawn_with_trigger(Arc::clone(&config))
            .expect("did you forget 'setcap cap_net_raw+ep'?");
    let net_monitor = Box::new(net_monitor);

    let observer = Observer::builder()
        .config(Arc::clone(&config))
        .sound(sound.clone())
        .led(led.clone())
        .main_mcu(main_mcu.clone())
        .net_monitor(net_monitor.clone())
        .build();
    let mut observer_task = DefaultObserverPlan::default().spawn(observer)?;

    led.bootup();
    let mut orb = Orb::builder()
        .config(Arc::clone(&config))
        .sound(sound.clone())
        .led(led.clone())
        .main_mcu(main_mcu)
        .net_monitor(net_monitor)
        .cpu_monitor(cpu_monitor)
        .build()
        .await?;

    setup_orb_token().await?;
    DATADOG.incr("orb.main.count.global.token_acquired", NO_TAGS)?;

    // Now that we connected to the WiFi, we can monitor our connection.
    net_monitor_trigger.fire();

    // In the current version, Python agents can be configured using the orb
    // configuration, but they must be configured before booting them as they
    // read their configuration from the config file stored in the local
    // filesystem. Therefore, we force the download of the latest configuration
    // from the backend and then store it locally. We don't fall back to the
    // stored version, since it's untrusted (in a writable partition).
    Config::download_and_store(Arc::clone(&config)).await?;
    warmup::Plan::default().run(&mut orb).await?;

    led.boot_complete();
    sound.build(sound::Type::Melody(Melody::BootUp))?.volume(0.5).push()?;

    let mut master_plan = {
        let mut builder = MasterPlan::builder();
        builder = builder
            .oneshot(cli.oneshot)
            .operator_qr_code(cli.operator_qr_code.as_ref().map(Option::as_deref))?
            .user_qr_code(cli.user_qr_code.as_ref().map(Option::as_deref))?;
        builder.build().await?
    };
    DATADOG.timing(
        "orb.main.time.global.init_main_program",
        t.elapsed().unwrap_or(Duration::MAX).as_millis().try_into()?,
        NO_TAGS,
    )?;

    let result = {
        let master_plan = master_plan.run(&mut orb).fuse();
        let ctrl_c = ctrl_c().fuse();
        pin_mut!(master_plan);
        pin_mut!(ctrl_c);
        select! {
            _ = (&mut observer_task).fuse() => Ok(false),
            result = master_plan => result,
            result = ctrl_c => {
                tracing::info!("Exiting on Ctrl-C");
                result.map(|()| true).map_err(Into::into)
            }
        }
    };
    observer_task.abort();
    master_plan.reset_hardware(&mut orb, Duration::from_millis(100)).await?;
    let _ = DATADOG.incr("orb.main.count.global.exiting_main_program", [format!(
        "exit_status:{}",
        result.is_ok()
    )]);
    match result {
        Ok(false) => orb.shutdown().await.map(|_| ()),
        Ok(true) => Ok(()),
        Err(err) => Err(err),
    }
}

async fn setup_orb_token() -> Result<()> {
    let token_timing = SystemTime::now();
    orb::short_lived_token::wait_for_token().await;
    tracing::debug!("Acquired orb token!");
    DATADOG.timing(
        "orb.main.time.global.short_lived_token.init",
        token_timing.elapsed().unwrap_or(Duration::MAX).as_millis().try_into()?,
        NO_TAGS,
    )?;

    // After the initial value, we still monitor changes to the token.
    tokio::task::spawn(orb::short_lived_token::monitor_token());

    Ok(())
}
