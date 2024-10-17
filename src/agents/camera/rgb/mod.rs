//! RGB camera sensor.

pub mod worker;

use self::worker::Worker;
pub use self::worker::{ArchivedFrame, Frame};
use super::Frame as _;
use crate::{dd_timing, ext::mpsc::SenderExt as _, image::fisheye, process::Command as StdCommand};
use agentwire::{
    agent::{self, Process as _},
    port::{self, Port},
};
use eyre::{bail, Error, Result, WrapErr};
use futures::{
    channel::mpsc,
    future::{self, BoxFuture, Either},
    prelude::*,
    select_biased,
};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{fmt, pin::pin, process::Stdio, time::Duration};
use tokio::{
    fs,
    io::{AsyncBufReadExt as _, BufReader},
    process::{self, ChildStderr, ChildStdout, Command as TokioCommand},
    time,
};
use tokio_stream::wrappers::IntervalStream;
use walkdir::WalkDir;

const NVARGUS_DAEMON: &str = "/usr/sbin/nvargus-daemon";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
const FRAME_TIMEOUT: Duration = Duration::from_millis(600);

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
    Start {
        /// Capture framerate.
        fps: u32,
    },
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
    worker_kill: agent::Kill,
    argus_error: mpsc::Receiver<()>,
}

impl Port for Sensor {
    type Input = Command;
    type Output = Frame;

    const INPUT_CAPACITY: usize = 100;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Sensor {
    const NAME: &'static str = "rgb-camera";
}

impl agentwire::agent::Task for Sensor {
    type Error = Error;

    async fn run(mut self, port: port::Inner<Self>) -> Result<(), Self::Error> {
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
        let (argus_error_tx, argus_error_rx) = mpsc::channel(1);
        let (worker, worker_kill) = Worker.spawn_process(worker_logger(argus_error_tx));
        Ok(Manager {
            port,
            state_tx,
            fisheye_config: None,
            fisheye_sent: false,
            nvargus,
            worker,
            worker_kill,
            argus_error: argus_error_rx,
        })
    }

    async fn run(mut self) -> Result<()> {
        while let Some(command) = self.port.next().await {
            match command.value {
                Command::Fisheye { fisheye_config: new_fisheye_config, undistortion_enabled } => {
                    self.fisheye_config = undistortion_enabled.then_some(new_fisheye_config);
                    self.fisheye_sent = false;
                }
                Command::Start { fps } => {
                    if let Some(state_tx) = &mut self.state_tx {
                        state_tx.send_now(super::State::Capturing)?;
                    }
                    let mut retry = true;
                    while retry {
                        (self, retry) = self.capture(fps).await?;
                    }
                    if let Some(state_tx) = &mut self.state_tx {
                        state_tx.send_now(super::State::Idle)?;
                    }
                }
                Command::Reset => {
                    (self, _) = self.worker_send_or_restart(worker::Command::Reset).await?;
                }
                Command::Stop => bail!("rgb camera already stopped"),
            }
        }
        Ok(())
    }

    async fn capture(mut self, fps: u32) -> Result<(Self, bool)> {
        macro_rules! worker_send_or_retry {
            ($command:expr) => {{
                let command = $command;
                let restarted;
                (self, restarted) = self.worker_send_or_restart(command).await?;
                if restarted {
                    return Ok((self, true));
                }
            }};
        }

        if !self.fisheye_sent {
            worker_send_or_retry!(worker::Command::FisheyeConfig(self.fisheye_config));
        }
        worker_send_or_retry!(worker::Command::Play(fps));
        self.fisheye_sent = true;
        let mut prev_timestamp = None;
        let mut last_frame_ts = time::Instant::now();
        let mut interval = time::interval_at(last_frame_ts + STARTUP_TIMEOUT, FRAME_TIMEOUT / 2);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut interval = IntervalStream::new(interval).fuse();
        loop {
            select_biased! {
                output = self.worker.next() => if let Some(output) = output {
                    last_frame_ts = time::Instant::now();
                    self.process_frame(output, &mut prev_timestamp)?;
                } else {
                    self = self.restart().await?;
                    return Ok((self, true));
                },
                command = self.port.next() => match command {
                    Some(command) => match command.value {
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
                    }
                    None => break,
                },
                tick = interval.next() => {
                    if tick.unwrap().saturating_duration_since(last_frame_ts) >= FRAME_TIMEOUT {
                        tracing::warn!("RGB camera frame timeout");
                        self = self.restart().await?;
                        return Ok((self, true));
                    }
                }
                _ = self.argus_error.next() => {
                    tracing::warn!("Argus error detected");
                    self = self.restart().await?;
                    return Ok((self, true));
                }
            }
        }
        (self, _) = self.worker_send_or_restart(worker::Command::Pause).await?;
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
            dd_timing!("main.time.camera.rgb_frame", delay);
        }
        *prev_timestamp = Some(timestamp);
        self.port.tx.send_now(derive(output.value))?;
        Ok(())
    }

    async fn worker_send_or_restart(mut self, command: worker::Command) -> Result<(Self, bool)> {
        let restarted =
            match time::timeout(STARTUP_TIMEOUT, self.worker.send(port::Input::new(command))).await
            {
                Err(_) => {
                    tracing::warn!("RGB camera command timeout");
                    self = self.restart().await?;
                    true
                }
                Ok(Err(err)) if err.is_disconnected() => {
                    self = self.restart().await?;
                    true
                }
                Ok(Err(_) | Ok(())) => false,
            };
        Ok((self, restarted))
    }

    async fn restart(mut self) -> Result<Self> {
        tracing::debug!("Restarting nvargus & gstreamer");
        if let Err(err) = self.nvargus.kill().await {
            tracing::warn!("nvargus-daemon kill failed: {err:?}");
        }
        self.worker_kill.await;
        self.nvargus = spawn_nvargus().await?;
        let (argus_error_tx, argus_error_rx) = mpsc::channel(1);
        (self.worker, self.worker_kill) = Worker.spawn_process(worker_logger(argus_error_tx));
        self.argus_error = argus_error_rx;
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
    TokioCommand::from(StdCommand::new(NVARGUS_DAEMON))
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

fn worker_logger(
    argus_error: mpsc::Sender<()>,
) -> impl Fn(&'static str, ChildStdout, ChildStderr) -> BoxFuture<()> + Send + 'static {
    move |agent_name, stdout, stderr| {
        let mut argus_error = argus_error.clone();
        Box::pin(async move {
            let mut stdout = BufReader::new(stdout).lines();
            let mut stderr = BufReader::new(stderr).lines();
            loop {
                match future::select(pin!(stdout.next_line()), pin!(stderr.next_line())).await {
                    Either::Left((Ok(Some(line)), _)) => {
                        tracing::info!("[{agent_name}] <STDOUT> {line}");
                    }
                    Either::Right((Ok(Some(line)), _)) => {
                        tracing::info!("[{agent_name}] <STDERR> {line}");
                        if line.starts_with("(Argus) Error ") {
                            argus_error.send(()).await.unwrap();
                        }
                    }
                    Either::Left((Ok(None), _)) => {
                        tracing::warn!("[{agent_name}] <STDOUT> closed");
                        break;
                    }
                    Either::Right((Ok(None), _)) => {
                        tracing::warn!("[{agent_name}] <STDERR> closed");
                        break;
                    }
                    Either::Left((Err(err), _)) => {
                        tracing::error!("[{agent_name}] <STDOUT> {err:#?}");
                        break;
                    }
                    Either::Right((Err(err), _)) => {
                        tracing::error!("[{agent_name}] <STDERR> {err:#?}");
                        break;
                    }
                }
            }
        })
    }
}
