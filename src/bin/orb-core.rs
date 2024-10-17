#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

use clap::Parser;
use eyre::{ensure, Result, WrapErr as _};
use futures::{pin_mut, prelude::*, select_biased};

#[cfg(feature = "internal-data-acquisition")]
use orb::logger::DATADOG_SUPPRESS;
use orb::{
    async_main,
    brokers::{DefaultObserverPlan, Observer, Orb},
    cli::Cli,
    config::Config,
    dd_incr, dd_timing, logger,
    mcu::{self, Mcu},
    monitor,
    plans::{warmup, MasterPlan},
    ui::{self, Engine},
};
#[cfg(feature = "internal-data-acquisition")]
use std::sync::atomic::Ordering;
use std::{
    convert::identity,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, SystemTime},
};
use tokio::{fs, signal::ctrl_c, sync::Mutex, task};

fn main() -> Result<()> {
    async_main(run(Cli::parse()))
}

#[allow(let_underscore_drop, clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<()> {
    logger::init::<false>();
    ensure!(sodiumoxide::init().is_ok(), "sodiumoxide initialization failure");
    #[cfg(feature = "internal-data-acquisition")]
    if cli.data_acquisition {
        DATADOG_SUPPRESS.store(true, Ordering::Relaxed);
    }

    dd_incr!("main.count.global.starting_main_program");
    let t = SystemTime::now();

    let ui = ui::Jetson::spawn();

    // When the orb boots up for the first time, there is no internet
    // connection, so we must rely solely on the local configuration. In any
    // case, this configuration setup is only used for playing basic startup
    // sounds.
    let config = if let Some(path) = &cli.config {
        serde_json::from_str(&fs::read_to_string(path).await?)?
    } else {
        Config::load_or_default().await
    };
    let config = Arc::new(Mutex::new(config));
    config.lock().await.propagate_to_ui(&ui);

    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());

    let main_mcu = Box::new(mcu::main::Jetson::spawn()?);
    let (net_monitor, net_monitor_trigger): (Box<dyn monitor::net::Monitor>, _) = 'net_monitor: {
        #[cfg(feature = "internal-data-acquisition")]
        if cli.data_acquisition {
            let (net_monitor, trigger) = monitor::net::Fake::spawn_with_trigger();
            break 'net_monitor (Box::new(net_monitor), trigger);
        }
        let (net_monitor, trigger) = monitor::net::Jetson::spawn_with_trigger(Arc::clone(&config))
            .expect("did you forget 'setcap cap_net_raw+ep'?");
        break 'net_monitor (Box::new(net_monitor), trigger);
    };

    let signup_flag = Arc::new(AtomicBool::new(false));
    let observer = Observer::builder()
        .config(Arc::clone(&config))
        .ui(ui.clone())
        .main_mcu(main_mcu.clone())
        .net_monitor(net_monitor.clone())
        .signup_flag(Arc::clone(&signup_flag))
        .build();
    let mut observer_task = task::spawn(DefaultObserverPlan::default().run(observer));

    let mut orb = Orb::builder()
        .config(Arc::clone(&config))
        .ui(ui.clone())
        .main_mcu(main_mcu)
        .net_monitor(net_monitor)
        .cpu_monitor(cpu_monitor)
        .build()
        .await?;
    #[cfg(feature = "livestream")]
    if cli.livestream {
        orb.enable_livestream()?;
    }

    setup_orb_token().await?;
    dd_incr!("main.count.global.token_acquired");

    // Now that we connected to the WiFi, we can monitor our connection.
    net_monitor_trigger.fire();

    // In the current version, Python agents can be configured using the orb
    // configuration, but they must be configured before booting them as they
    // read their configuration from the config file stored in the local
    // filesystem. Therefore, we force the download of the latest configuration
    // from the backend and then store it locally. We don't fall back to the
    // stored version, since it's untrusted (in a writable partition).
    'config_download: {
        #[cfg(feature = "internal-data-acquisition")]
        if cli.data_acquisition {
            break 'config_download;
        }
        if cli.config.is_some() {
            break 'config_download;
        }
        break 'config_download Config::download_and_store(Arc::clone(&config)).await?;
    }
    warmup::Plan::default().run(&mut orb).await?;

    ui.boot_complete(false);

    let mut master_plan = {
        let mut builder = MasterPlan::builder();
        builder = builder
            .oneshot(cli.oneshot)
            .operator_qr_code(cli.operator_qr_code.as_ref().map(Option::as_deref))?
            .user_qr_code(cli.user_qr_code.as_ref().map(Option::as_deref))?
            .signup_flag(signup_flag);
        #[cfg(feature = "allow-plan-mods")]
        {
            builder = builder
                .skip_pipeline(cli.skip_pipeline)
                .skip_fraud_checks(cli.skip_fraud_checks)
                .biometric_input(cli.biometric_input);
        }
        #[cfg(feature = "integration_testing")]
        {
            builder = builder.ci_hacks(cli.ci_hacks);
        }
        #[cfg(feature = "internal-data-acquisition")]
        {
            builder = builder.data_acquisition(cli.data_acquisition);
        }
        builder.build().await?
    };
    dd_timing!("main.time.global.init_main_program", t);

    let result = {
        let master_plan = master_plan.run(&mut orb).fuse();
        let ctrl_c = ctrl_c().fuse();
        pin_mut!(master_plan);
        pin_mut!(ctrl_c);
        select_biased! {
            result = (&mut observer_task).fuse() => {
                result.wrap_err("observer task").map_err(Into::into).and_then(identity)
            }
            result = master_plan => {
                result
            }
            result = ctrl_c => {
                tracing::info!("Exiting on Ctrl-C");
                result.map_err(Into::into)
            }
        }
    };
    observer_task.abort();
    master_plan.reset_hardware(&mut orb, Duration::from_millis(100)).await?;
    dd_incr!("main.count.global.exiting_main_program", &format!("exit_status:{}", result.is_ok()));
    result
}

async fn setup_orb_token() -> Result<()> {
    let token_timing = SystemTime::now();
    orb::short_lived_token::wait_for_token().await;
    tracing::debug!("Acquired orb token!");
    dd_timing!("main.time.global.short_lived_token.init", token_timing);

    // After the initial value, we still monitor changes to the token.
    task::spawn(orb::short_lived_token::monitor_token());

    Ok(())
}
