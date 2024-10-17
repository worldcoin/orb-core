use eyre::Result;
use orb::{
    async_main,
    brokers::Orb,
    config::Config,
    dd_incr, logger,
    mcu::{self, Mcu},
    monitor,
    plans::wifi,
    ui::{self, Engine},
};
use std::sync::Arc;
use tokio::sync::Mutex;

fn main() -> Result<()> {
    async_main(run())
}

async fn run() -> Result<()> {
    logger::init::<false>();
    let ui = ui::Jetson::spawn();
    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    config.lock().await.propagate_to_ui(&ui);
    let main_mcu = Box::new(mcu::main::Jetson::spawn()?);
    let net_monitor = monitor::net::Jetson::spawn(Arc::clone(&config))
        .expect("did you forget 'setcap cap_net_raw+ep'?");
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    ui.bootup();

    let mut orb = Orb::builder()
        .config(Arc::clone(&config))
        .ui(ui.clone())
        .main_mcu(main_mcu.clone())
        .net_monitor(Box::new(net_monitor))
        .cpu_monitor(cpu_monitor)
        .build()
        .await?;

    wifi::Plan.ensure_network_connection(&mut orb).await?;
    dd_incr!("main.count.global.network_connected");
    Ok(())
}
