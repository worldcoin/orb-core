//! RGB camera sensor.

pub mod worker;

use self::worker::Worker;
pub use self::worker::{ArchivedFrame, Frame};
use super::Frame as _;
use crate::{
    agents::{AgentKill, AgentProcess},
    ext::mpsc::SenderExt as _,
    fisheye,
    logger::{DATADOG, NO_TAGS},
    port,
    port::Port,
};
use async_trait::async_trait;
use eyre::{bail, Result, WrapErr};
use futures::{channel::mpsc, future, future::Either, prelude::*};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{convert::TryInto, fmt, process::Stdio, time::Duration};
use tokio::{fs, process};
use walkdir::WalkDir;

const NVARGUS_DAEMON: &str = "/usr/sbin/nvargus-daemon";

/// RGB camera sensor.
///
/// See [the module-level documentation](self) for details.
pub struct Sensor {
    state_tx: Option<mpsc::Sender<super::State>>,
    fake_port: Option<port::Outer<Sensor>>,
}

impl fmt::Debug for Sensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("camera::rgb::Sensor").finish_non_exhaustive()
    }
}

impl Sensor {
    /// RGB camera mounted in the front of the Orb.
    #[must_use]
    pub fn new(
        state_tx: Option<mpsc::Sender<super::State>>,
        fake_port: Option<port::Outer<Sensor>>,
    ) -> Self {
        Self { state_tx, fake_port }
    }
}

/// Sensor commands.
#[derive(Debug)]
pub enum Command {
    /// Set fisheye configuration.
    Fisheye {
        /// Fisheye configuration.
        fisheye_config: fisheye::Config,
        /// Whether to undistort frames.
        undistortion_enabled: bool,
    },
    /// Start frame capturing.
    Start,
    /// Stop frame capturing.
    Stop,
    /// Ensure no stale frames leak into the next capture by fully restarting
    /// the internal gstreamer pipeline.
    Reset,
}

struct Manager {
    port: port::Inner<Sensor>,
    state_tx: Option<mpsc::Sender<super::State>>,
    fisheye_config: Option<fisheye::Config>,
    fisheye_sent: bool,
    nvargus: process::Child,
    worker: port::Outer<Worker>,
    worker_kill: AgentKill,
}

impl Port for Sensor {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 100;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Sensor {
    const NAME: &'static str = "rgb-camera";
}

#[async_trait]
impl super::AgentTask for Sensor {
    async fn run(mut self, port: port::Inner<Self>) -> Result<()> {
        if let Some(fake_port) = self.fake_port.take() {
            run_fake(port, fake_port).await
        } else {
            Manager::new(port, self.state_tx).await?.run().await
        }
    }
}

impl Manager {
    async fn new(
        port: port::Inner<Sensor>,
        state_tx: Option<mpsc::Sender<super::State>>,
    ) -> Result<Self> {
        let nvargus = spawn_nvargus().await?;
        let (worker, worker_kill) = Worker.spawn_process();
        Ok(Manager {
            port,
            state_tx,
            fisheye_config: None,
            fisheye_sent: false,
            nvargus,
            worker,
            worker_kill,
        })
    }

    async fn run(mut self) -> Result<()> {
        while let Some(command) = self.port.next().await {
            match command.value {
                Command::Fisheye { fisheye_config: new_fisheye_config, undistortion_enabled } => {
                    self.fisheye_config = undistortion_enabled.then_some(new_fisheye_config);
                    self.fisheye_sent = false;
                }
                Command::Start => {
                    if let Some(state_tx) = &mut self.state_tx {
                        state_tx.send_now(super::State::Capturing)?;
                    }
                    let mut retry = true;
                    while retry {
                        (self, retry) = self.capture().await?;
                    }
                    if let Some(state_tx) = &mut self.state_tx {
                        state_tx.send_now(super::State::Idle)?;
                    }
                }
                Command::Reset => {
                    self = self.worker_send_or_restart(worker::Command::Reset).await?;
                }
                Command::Stop => bail!("rgb camera already stopped"),
            }
        }
        Ok(())
    }

    async fn capture(mut self) -> Result<(Self, bool)> {
        macro_rules! worker_send_or_retry {
            ($command:expr) => {
                match self.worker.send(port::Input::new($command)).await {
                    Err(err) if err.is_disconnected() => {
                        self = self.restart().await?;
                        return Ok((self, true));
                    }
                    _ => {}
                }
            };
        }

        if !self.fisheye_sent {
            worker_send_or_retry!(worker::Command::FisheyeConfig(self.fisheye_config));
        }
        worker_send_or_retry!(worker::Command::Play);
        self.fisheye_sent = true;
        let mut prev_timestamp = None;
        loop {
            match future::select(self.worker.next(), self.port.next()).await {
                Either::Left((Some(output), _)) => {
                    self.process_frame(output, &mut prev_timestamp)?;
                }
                Either::Right((Some(command), _)) => match command.value {
                    Command::Fisheye {
                        fisheye_config: new_fisheye_config,
                        undistortion_enabled,
                    } => {
                        self.fisheye_config = undistortion_enabled.then_some(new_fisheye_config);
                        worker_send_or_retry!(worker::Command::FisheyeConfig(self.fisheye_config));
                    }
                    Command::Start { .. } | Command::Reset => {
                        bail!("rgb camera already started")
                    }
                    Command::Stop => break,
                },
                Either::Left((None, _)) => {
                    self = self.restart().await?;
                    return Ok((self, true));
                }
                Either::Right((None, _)) => break,
            }
        }
        self = self.worker_send_or_restart(worker::Command::Pause).await?;
        Ok((self, false))
    }

    fn process_frame(
        &mut self,
        output: port::Output<Worker>,
        prev_timestamp: &mut Option<Duration>,
    ) -> Result<()> {
        let derive = output.derive_fn();
        let timestamp = output.value.timestamp();
        if let Some(delay) = prev_timestamp.and_then(|prev| timestamp.checked_sub(prev)) {
            DATADOG.timing(
                "orb.main.time.camera.rgb_frame",
                delay.as_millis().try_into()?,
                NO_TAGS,
            )?;
        }
        *prev_timestamp = Some(timestamp);
        self.port.send_now(derive(output.value))?;
        Ok(())
    }

    async fn worker_send_or_restart(mut self, command: worker::Command) -> Result<Self> {
        match self.worker.send(port::Input::new(command)).await {
            Err(err) if err.is_disconnected() => {
                self = self.restart().await?;
            }
            _ => {}
        }
        Ok(self)
    }

    async fn restart(mut self) -> Result<Self> {
        tracing::debug!("Restarting nvargus");
        if let Err(err) = self.nvargus.kill().await {
            tracing::warn!("nvargus-daemon kill failed: {err:?}");
        }
        self.worker_kill.await;
        self.nvargus = spawn_nvargus().await?;
        (self.worker, self.worker_kill) = Worker.spawn_process();
        self.fisheye_sent = false;
        Ok(self)
    }
}

async fn spawn_nvargus() -> Result<process::Child> {
    // Kill previously running daemons first.
    for entry in WalkDir::new("/proc").min_depth(2).max_depth(2) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!("Error traversing /proc: {err:?}");
                continue;
            }
        };
        if entry.file_name() != "cmdline" {
            continue;
        }
        match fs::read_to_string(entry.path()).await {
            Ok(string) if !string.starts_with(NVARGUS_DAEMON) => {
                continue;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("Error reading contents of {}: {err}", entry.path().display());
                continue;
            }
        }
        let pid = entry.path().parent().unwrap().file_name().unwrap();
        if let Ok(pid) = pid.to_string_lossy().parse::<i32>() {
            tracing::warn!("Killing abandoned nvargus-daemon with PID {pid}");
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
    process::Command::new(NVARGUS_DAEMON)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .wrap_err("failed spawning nvargus-daemon")
}

async fn run_fake(mut port: port::Inner<Sensor>, mut fake_port: port::Outer<Sensor>) -> Result<()> {
    loop {
        match future::select(port.next(), fake_port.next()).await {
            Either::Left((Some(command), _)) => {
                let _ = fake_port.send(command).await;
            }
            Either::Right((Some(fake_output), _)) => {
                port.send(fake_output).await?;
            }
            Either::Left((None, _)) => {
                break;
            }
            Either::Right((None, _)) => {}
        }
    }
    Ok(())
}
