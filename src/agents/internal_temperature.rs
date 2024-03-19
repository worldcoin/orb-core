//! Temperature sensors.

#![allow(clippy::similar_names)] // triggered on `cpu` and `gpu`

use crate::{port, port::Port};
use async_trait::async_trait;
use eyre::Result;
use futures::prelude::*;
use std::{convert::Infallible, time::Duration};
use tokio::{fs, time};
use tokio_stream::wrappers::IntervalStream;

const READ_INTERVAL: Duration = Duration::from_millis(1000);
const CPU_PATH: &str = "/sys/class/thermal/thermal_zone1/temp";
const GPU_PATH: &str = "/sys/class/thermal/thermal_zone2/temp";
const SSD_PATH: &str = "/sys/class/nvme/nvme0/device/hwmon/hwmon2/temp1_input";

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
}

impl Port for Sensor {
    type Input = Infallible;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Sensor {
    const NAME: &'static str = "internal-temperature";
}

#[async_trait]
impl super::AgentTask for Sensor {
    async fn run(self, mut port: port::Inner<Self>) -> Result<()> {
        if !fs::try_exists(SSD_PATH).await? {
            tracing::error!("SSD temperature not available you need to update your kernel");
        }

        let mut interval = time::interval(READ_INTERVAL);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut interval = IntervalStream::new(interval).fuse();
        while interval.next().await.is_some() {
            let cpu = read_temperature(CPU_PATH).await?;
            let gpu = read_temperature(GPU_PATH).await?;
            let ssd = read_temperature(SSD_PATH).await?;
            if port.send(port::Output::new(Output { cpu, gpu, ssd })).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]
async fn read_temperature(path: &str) -> Result<i16> {
    if fs::try_exists(path).await? {
        let temp = fs::read_to_string(path).await?.trim_end().parse::<i64>()? as f64 / 1000.0;
        let temp = temp.clamp(i16::MIN.into(), i16::MAX.into()).round() as i16;
        Ok(temp)
    } else {
        Ok(i16::MIN)
    }
}
