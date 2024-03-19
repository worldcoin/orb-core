#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

use eyre::Result;
use orb::{
    config::Config,
    led::{self, Engine},
    mcu::{self, Mcu},
    monitor::{self, cpu::Monitor as _},
    sound::{self, Player, Type, Voice},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let mut sound = sound::Jetson::spawn(config, true, cpu_monitor.clone()).await?;
    let main_mcu = mcu::main::Jetson::spawn()?;
    let led = led::Jetson::spawn(main_mcu.clone());
    led.recovery();

    loop {
        sound.build(Type::Voice(Voice::PleaseDontShutDown))?.volume(0.5).push()?;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
