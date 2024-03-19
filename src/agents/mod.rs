//! Collection of agents.
//!
//! See [the brokers module documentation](crate::brokers) for details.
//!
//! # Examples
//!
//! ```
//! # #[tokio::main] async fn main() {
//! use async_trait::async_trait;
//! use eyre::Result;
//! use futures::{channel::mpsc, prelude::*};
//! use orb::{
//!     agents::{Agent, AgentTask},
//!     port,
//!     port::Port,
//! };
//!
//! /// An agent that receives numbers, multiplies them by 2, and sends them
//! /// back.
//! struct Doubler;
//!
//! impl Port for Doubler {
//!     type Input = u32;
//!     type Output = u32;
//!
//!     const INPUT_CAPACITY: usize = 0;
//!     const OUTPUT_CAPACITY: usize = 0;
//! }
//!
//! impl Agent for Doubler {
//!     const NAME: &'static str = "doubler";
//! }
//!
//! #[async_trait]
//! impl AgentTask for Doubler {
//!     async fn run(self, mut port: port::Inner<Self>) -> Result<()> {
//!         while let Some(x) = port.next().await {
//!             port.send(x.chain(x.value * 2)).await?;
//!         }
//!         Ok(())
//!     }
//! }
//!
//! let (mut doubler, _kill) = Doubler.spawn_task();
//!
//! // Send an input message to the agent.
//! doubler.send(port::Input::new(3)).await;
//! // Receive an output message from the agent.
//! let output = doubler.next().await;
//! assert_eq!(output.unwrap().value, 6);
//! # }
//! ```

/// Takes a process-based agent name, and returns its `call` function.
///
/// # Panics
///
/// If `name` is unknown.
// NOTE: keep track of all process-based agents here!
#[must_use]
pub fn agent_process_map(name: &str) -> fn(OwnedFd) -> Result<()> {
    match name {
        "mega-agent-one" => python::mega_agent_one::MegaAgentOne::call,
        "mega-agent-two" => python::mega_agent_two::MegaAgentTwo::call,
        "rgb-camera-worker" => camera::rgb::worker::Worker::call,
        "thermal-camera" => camera::thermal::Sensor::call,
        "qr-code" => qr_code::Agent::call,
        _ => panic!("unregistered agent {name}"),
    }
}

pub mod camera;
pub mod distance;
pub mod eye_pid_controller;
pub mod eye_tracker;
pub mod image_notary;
pub mod image_uploader;
pub mod internal_temperature;
pub mod ir_auto_exposure;
pub mod ir_auto_focus;
pub mod mirror;
pub mod python;
pub mod qr_code;
pub mod thermal;

use crate::{
    brokers::AgentKill,
    logger,
    port::{self, Port, SharedPort, SharedSerializer},
    utils::{set_proc_name, spawn_named_thread},
};
use async_trait::async_trait;
use close_fds::close_open_fds;
use eyre::Result;
use futures::{future::Either, prelude::*};
use nix::{
    sched::{unshare, CloneFlags},
    sys::signal::{self, Signal},
    unistd::Pid,
};
use rkyv::{de::deserializers::SharedDeserializeMap, Archive, Deserialize, Infallible, Serialize};
use std::{
    env,
    fmt::Debug,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::process::{parent_id, ExitStatusExt},
    },
    process,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    runtime,
    sync::oneshot,
    task,
};

const PROCESS_NAME_ENV: &str = "ORB_CORE_PROCESS_NAME";
const PROCESS_SHMEM_ENV: &str = "ORB_CORE_PROCESS_SHMEM";
const PROCESS_PARENT_PID_ENV: &str = "ORB_CORE_PROCESS_PARENT_PID";
#[doc(hidden)]
pub const PROCESS_ARGS_ENV: &str = "ORB_CORE_PROCESS_ARGS";

static INIT_PROCESSES: AtomicBool = AtomicBool::new(false);

/// Abstract agent.
pub trait Agent: Port + Sized + 'static {
    /// Name of the agent. Must be unique.
    const NAME: &'static str;
}

/// Agent running on a dedicated asynchronous task.
#[async_trait]
pub trait AgentTask: Agent + Send {
    /// Runs the agent event-loop inside a dedicated asynchronous task.
    async fn run(self, port: port::Inner<Self>) -> Result<()>;

    /// Spawns a new task running the agent event-loop and returns a handle for
    /// bi-directional communication with the agent.
    fn spawn_task(self) -> (port::Outer<Self>, AgentKill) {
        let (inner, outer) = port::new();
        task::spawn(async move {
            tracing::info!("Agent {} spawned", Self::NAME);
            match self.run(inner).await {
                Ok(()) => {
                    tracing::warn!("Task agent {} exited", Self::NAME);
                }
                Err(err) => {
                    tracing::error!("Task agent {} exited with error: {:?}", Self::NAME, err);
                }
            }
        });
        (outer, future::pending().boxed())
    }
}

/// Agent running on a dedicated OS thread.
pub trait AgentThread: Agent + Send {
    /// Runs the agent event-loop inside a dedicated OS thread.
    fn run(self, port: port::Inner<Self>) -> Result<()>;

    /// Spawns a new thread running the agent event-loop and returns a handle for
    /// bi-directional communication with the agent.
    fn spawn_thread(self) -> io::Result<(port::Outer<Self>, AgentKill)> {
        let (inner, outer) = port::new();
        spawn_named_thread(format!("thrd-{}", Self::NAME), move || {
            tracing::info!("Agent {} spawned", Self::NAME);
            match self.run(inner) {
                Ok(()) => {
                    tracing::warn!("Thread agent {} exited", Self::NAME);
                }
                Err(err) => {
                    tracing::error!("Thread agent {} exited with error: {:?}", Self::NAME, err);
                }
            }
        });
        Ok((outer, future::pending().boxed()))
    }
}

/// Agent running on a dedicated OS process.
///
/// NOTE: When implementing this trait, add a new match arm to the
/// [`agent_process_map`].
pub trait AgentProcess
where
    Self: Agent
        + SharedPort
        + Clone
        + Send
        + Debug
        + Archive
        + for<'a> Serialize<SharedSerializer<'a>>,
    <Self as Archive>::Archived: Deserialize<Self, Infallible>,
    Self::Input: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    Self::Output: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    <Self::Output as Archive>::Archived: Deserialize<Self::Output, SharedDeserializeMap>,
{
    /// Runs the agent event-loop inside a dedicated OS thread.
    fn run(self, port: port::RemoteInner<Self>) -> Result<()>;

    /// Spawns a new process running the agent event-loop and returns a handle
    /// for bi-directional communication with the agent.
    ///
    /// # Panics
    ///
    /// If [`init_processes`] hasn't been called yet.
    fn spawn_process(self) -> (port::Outer<Self>, AgentKill) {
        assert!(
            INIT_PROCESSES.load(Ordering::Relaxed),
            "process-based agents are not initialized (missing call to `agents::init_processes`)"
        );
        let (inner, outer) = port::new();
        let (send_kill_tx, send_kill_rx) = oneshot::channel();
        let (wait_kill_tx, wait_kill_rx) = oneshot::channel();
        let kill = async move {
            let _ = send_kill_tx.send(());
            wait_kill_rx.await.unwrap();
            tracing::info!("Process agent {} killed", Self::NAME);
        };
        let spawn_process = spawn_process_impl(self, inner, send_kill_rx, wait_kill_tx);
        spawn_named_thread(format!("proc-ipc-{}", Self::NAME), || {
            let rt = runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(task::LocalSet::new().run_until(spawn_process));
        });
        (outer, kill.boxed())
    }

    /// Connects to the shared memory and calls the [`run`](Self::run) method.
    fn call(shmem: OwnedFd) -> Result<()> {
        let mut inner = port::RemoteInner::<Self>::from_shared_memory(shmem)?;
        let agent = inner.init_state().deserialize(&mut Infallible).unwrap();
        agent.run(inner)
    }

    /// Additional environment variables for the process.
    #[must_use]
    fn envs() -> Vec<(String, String)> {
        Vec::new()
    }

    /// When the agent process terminates, this method decides how to proceed.
    /// See [`AgentProcessExitStrategy`] for available options.
    #[must_use]
    fn exit_strategy(_code: Option<i32>, _signal: Option<i32>) -> AgentProcessExitStrategy {
        AgentProcessExitStrategy::default()
    }
}

/// Exit strategy returned from [`AgentProcess::exit_strategy`].
#[derive(Clone, Copy, Default, Debug)]
pub enum AgentProcessExitStrategy {
    /// Close the port without restarting the agent.
    Close,
    /// Keep the port open and restart the agent.
    Restart,
    /// Keep the port open, restart the agent, and retry the latest input.
    #[default]
    Retry,
}

/// Initializes process-based agents.
///
/// This function must be called as early in the program lifetime as possible.
/// Everything before this function call gets duplicated for each process-based
/// agent.
pub fn init_processes() {
    let env_vars =
        (env::var(PROCESS_NAME_ENV), env::var(PROCESS_SHMEM_ENV), env::var(PROCESS_PARENT_PID_ENV));
    match env_vars {
        (Ok(name), Ok(shmem), Ok(parent_pid)) => {
            unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
            set_proc_name(format!("proc-{name}"));
            if parent_id() != parent_pid.parse::<u32>().unwrap() {
                // The parent exited before the above `prctl` call.
                process::exit(1);
            }
            logger::init_for_agent();
            let shmem_fd = unsafe {
                OwnedFd::from_raw_fd(
                    shmem.parse::<RawFd>().expect("shared memory file descriptor to be an integer"),
                )
            };
            match agent_process_map(&name)(shmem_fd) {
                Ok(()) => {
                    tracing::warn!("Agent {name} exited");
                }
                Err(err) => {
                    tracing::error!("Agent {name} exited with an error: {err:#?}");
                }
            }
            process::exit(1);
        }
        (Err(_), Err(_), Err(_)) => {
            INIT_PROCESSES.store(true, Ordering::Relaxed);
        }
        (name, shmem, parent_pid) => {
            panic!(
                "Inconsistent state of the following environment variables: \
                 {PROCESS_NAME_ENV}={name:?}, {PROCESS_SHMEM_ENV}={shmem:?}, \
                 {PROCESS_PARENT_PID_ENV}={parent_pid:?}, "
            );
        }
    }
}

fn sandbox_agent() -> std::io::Result<()> {
    match unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWIPC) {
        Ok(()) => Ok(()),
        #[cfg(feature = "stage")]
        Err(nix::errno::Errno::EINVAL) => {
            tracing::warn!(
                "Failed to unshare, Kernel does not support it. That is ok for 'stage' but will \
                 be fatal in 'prod'"
            );
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

async fn spawn_process_impl<T: AgentProcess>(
    init_state: T,
    mut inner: port::Inner<T>,
    mut send_kill_rx: oneshot::Receiver<()>,
    wait_kill_tx: oneshot::Sender<()>,
) where
    <T as Archive>::Archived: Deserialize<T, Infallible>,
    T::Input: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    T::Output: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    <T::Output as Archive>::Archived: Deserialize<T::Output, SharedDeserializeMap>,
{
    let mut recovered_inputs = Vec::new();
    loop {
        let (shmem_fd, close) = inner
            .into_shared_memory(T::NAME, &init_state, recovered_inputs)
            .expect("couldn't initialize shared memory");
        let exe = env::current_exe().expect("couldn't determine current executable file");

        let child_fd = shmem_fd.as_raw_fd();
        let mut child = unsafe {
            Command::new(exe)
                .arg0(format!("proc-{}", T::NAME))
                .args(
                    &env::var(PROCESS_ARGS_ENV)
                        .map(|args| shell_words::split(&args).expect("invalid process arguments"))
                        .unwrap_or_default(),
                )
                .envs(T::envs())
                .env(PROCESS_NAME_ENV, T::NAME)
                .env(PROCESS_SHMEM_ENV, shmem_fd.as_raw_fd().to_string())
                .env(PROCESS_PARENT_PID_ENV, process::id().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .pre_exec(sandbox_agent)
                .pre_exec(move || {
                    close_open_fds(libc::STDERR_FILENO + 1, &[child_fd]);
                    Ok(())
                })
                .spawn()
                .expect("failed to spawn a sub-process")
        };
        drop(shmem_fd);
        let pid = Pid::from_raw(child.id().unwrap().try_into().unwrap());
        spawn_child_logger(T::NAME, "STDOUT", child.stdout.take().unwrap());
        spawn_child_logger(T::NAME, "STDERR", child.stderr.take().unwrap());
        tracing::info!("Process agent {} spawned with PID: {}", T::NAME, pid.as_raw());
        match future::select(Box::pin(child.wait()), &mut send_kill_rx).await {
            Either::Left((status, _)) => {
                let status = status.expect("failed to run a sub-process");
                let (code, signal) = (status.code(), status.signal());
                if signal.is_some_and(|signal| signal == 2) {
                    tracing::warn!("Process agent {} exited on Ctrl-C", T::NAME);
                    break;
                }
                let exit_strategy = T::exit_strategy(code, signal);
                tracing::info!(
                    "Process agent {} exited with code {code:?} and signal {signal:?}, proceeding \
                     with {exit_strategy:?}",
                    T::NAME
                );
                (inner, recovered_inputs) =
                    close.await.expect("shared memory deinitialization failure");
                match exit_strategy {
                    AgentProcessExitStrategy::Close => {
                        let _ = wait_kill_tx.send(());
                        break;
                    }
                    AgentProcessExitStrategy::Restart => {
                        recovered_inputs.clear();
                    }
                    AgentProcessExitStrategy::Retry => {}
                }
            }
            Either::Right((_kill, wait)) => {
                signal::kill(pid, Signal::SIGKILL)
                    .expect("failed to send SIGKILL to a sub-process");
                wait.await.expect("failed to kill a sub-process");
                close.await.expect("shared memory deinitialization failure");
                let _ = wait_kill_tx.send(());
                break;
            }
        };
    }
}

fn spawn_child_logger(
    agent_name: &'static str,
    output_name: &'static str,
    output: impl AsyncRead + Send + Unpin + 'static,
) {
    task::spawn(async move {
        let mut output = BufReader::new(output).lines();
        loop {
            match output.next_line().await {
                Ok(Some(line)) => {
                    tracing::info!("[{agent_name}] <{output_name}> {line}");
                }
                Ok(None) => {
                    tracing::warn!("[{agent_name}] <{output_name}> closed");
                    break;
                }
                Err(err) => {
                    tracing::error!("[{agent_name}] <{output_name}> {err:#?}");
                    break;
                }
            }
        }
    });
}

/// Polls the port for commands, finishing when there are no pending commands.
#[macro_export]
macro_rules! poll_commands {
    (|$port:ident, $cx:ident| $($clauses:tt)*) => {
        while let Poll::Ready(command) = Pin::new(&mut $port).poll_next($cx) {
            $crate::command!(|command| $($clauses)*);
        }
    };
}

/// Handles a command.
#[macro_export]
macro_rules! command {
    (|$command:ident| $($clauses:tt)*) => {
        #[allow(unreachable_patterns)]
        match $command.map(|x| x.value) {
            $($clauses)*
            command => ::eyre::bail!("unexpected command {:?}", command),
        }
    };
}

/// # Panics
/// If the plaintext and ciphertext are the same
#[cfg(not(feature = "no-image-encryption"))]
fn encrypt_and_seal(plaintext: &[u8]) -> Vec<u8> {
    // encrypt contents
    use sodiumoxide::crypto::sealedbox;
    let ciphertext = sealedbox::seal(plaintext, &crate::consts::WORLDCOIN_ENCRYPTION_PUBKEY);
    assert_ne!(plaintext, ciphertext);
    ciphertext
}
