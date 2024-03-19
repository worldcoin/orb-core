use eyre::Result;
use orb::{
    async_main,
    brokers::Orb,
    config::Config,
    led::{self, Engine},
    logger,
    logger::{DATADOG, NO_TAGS},
    mcu::{self, Mcu},
    monitor::{self, cpu::Monitor as _},
    plans::wifi,
    sound::{self, Player},
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn main() -> Result<()> {
    async_main(run())
}

async fn run() -> Result<()> {
    logger::init::<false>();
    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    let main_mcu = Box::new(mcu::main::Jetson::spawn()?);
    let led = led::Jetson::spawn(main_mcu.clone());
    let net_monitor = monitor::net::Jetson::spawn(Arc::clone(&config))
        .expect("did you forget 'setcap cap_net_raw+ep'?");
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let sound = sound::Jetson::spawn(Arc::clone(&config), true, cpu_monitor.clone()).await?;
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());

    led.bootup();

    let mut orb = Orb::builder()
        .config(Arc::clone(&config))
        .sound(sound.clone())
        .led(led.clone())
        .main_mcu(main_mcu.clone())
        .net_monitor(Box::new(net_monitor))
        .cpu_monitor(cpu_monitor)
        .build()
        .await?;

    wifi::Plan::new().ensure_network_connection(&mut orb).await?;
    DATADOG.incr("orb.main.count.global.network_connected", NO_TAGS)?;

    Ok(())
}
