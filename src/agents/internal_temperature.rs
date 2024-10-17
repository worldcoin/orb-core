//! Temperature sensors.

#![allow(clippy::similar_names)] // triggered on `cpu` and `gpu`

use crate::identification;
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::prelude::*;
use std::{convert::Infallible, ffi::OsString, path::Path, time::Duration};
use tokio::{fs, time};
use tokio_stream::wrappers::IntervalStream;

const READ_INTERVAL: Duration = Duration::from_millis(1000);
const CPU_PATH: &str = "/sys/class/thermal/thermal_zone1/temp";
const GPU_PATH: &str = "/sys/class/thermal/thermal_zone2/temp";
const SSD_PATH: &str = "/sys/class/nvme/nvme0/device/hwmon/hwmon2/temp1_input";
const HWMON_DIR_PATH: &str = "/sys/class/hwmon/";
const WIFI_DEVICE: &str = "iwlwifi_1";

/// Temperature sensors.
#[derive(Debug)]
pub struct Sensor;

/// Sensor output.
#[derive(Debug)]
pub struct Output {
    /// CPU temperature
    pub cpu: i16,
    /// GPU temperature
    pub gpu: i16,
    /// SSD temperature
    pub ssd: i16,
    /// WiFi module temperature
    pub wifi: Option<i16>,
}

impl Port for Sensor {
    type Input = Infallible;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Sensor {
    const NAME: &'static str = "internal-temperature";
}

impl agentwire::agent::Task for Sensor {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        log_ssd_state().await;

        let hwmon_iwlwifi_path = find_wifi_temperature_file().await;

        let mut interval = time::interval(READ_INTERVAL);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut interval = IntervalStream::new(interval).fuse();
        while interval.next().await.is_some() {
            let cpu = read_temperature_with_logging(CPU_PATH).await;
            let gpu = read_temperature_with_logging(GPU_PATH).await;
            let ssd = read_temperature_with_logging(SSD_PATH).await;
            let wifi = if let Some(hwmon_iwlwifi_path) = &hwmon_iwlwifi_path {
                Some(read_temperature_with_logging(hwmon_iwlwifi_path).await)
            } else {
                None
            };
            if port.send(port::Output::new(Output { cpu, gpu, ssd, wifi })).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]
async fn read_temperature<P: AsRef<Path>>(path: P) -> Result<i16> {
    let path = path.as_ref();
    if fs::try_exists(path).await? {
        let temp = fs::read_to_string(path).await?.trim_end().parse::<i64>()? as f64 / 1000.0;
        let temp = temp.clamp(i16::MIN.into(), i16::MAX.into()).round() as i16;
        Ok(temp)
    } else {
        Ok(i16::MIN)
    }
}

async fn read_temperature_with_logging<P: AsRef<Path>>(path: P) -> i16 {
    read_temperature(&path).await.unwrap_or_else(|e| {
        let path_str = path.as_ref().display();
        tracing::error!("Failed to read {path_str} temperature: {e}");
        i16::MIN
    })
}

async fn log_ssd_state() {
    if identification::HARDWARE_VERSION.contains("Diamond") {
        return;
    }
    match fs::try_exists(SSD_PATH).await {
        Ok(true) => {}
        Ok(false) => tracing::error!("SSD temperature not available. Update your kernel?"),
        Err(e) => tracing::error!("Failed to check SSD ({SSD_PATH}) temperature: {e}"),
    }
}

/// Find file keeping track of wifi module temperature
async fn find_wifi_temperature_file() -> Option<OsString> {
    match fs::read_dir(HWMON_DIR_PATH).await {
        Ok(mut entries) => loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => return None,
                Err(e) => {
                    tracing::error!("Failed to read hwmon directory entries ({entries:?}): {e}");
                    continue;
                }
            };

            let name_file = &entry.path().join("name");
            match fs::read_to_string(name_file).await {
                Ok(name) => {
                    if name.trim_end() == WIFI_DEVICE {
                        let path = entry.path().join("temp1_input");
                        tracing::info!("Found wifi card temperature into: {}", path.display());
                        return Some(path.into_os_string());
                    }
                }
                Err(e) => tracing::error!("Failed to read hwmon name file {name_file:?}: {e}"),
            }
        },
        Err(e) => {
            tracing::error!("Failed to read hwmon ({HWMON_DIR_PATH}) directory: {e}");
            None
        }
    }
}
