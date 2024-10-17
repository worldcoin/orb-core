//! Collection of agents.
//!
//! See [the brokers module documentation](crate::brokers) for details.

pub mod camera;
pub mod data_uploader;
pub mod distance;
pub mod eye_pid_controller;
pub mod eye_tracker;
pub mod image_notary;
#[cfg(feature = "internal-data-acquisition")]
pub mod image_uploader;
pub mod internal_temperature;
pub mod ir_auto_exposure;
pub mod ir_auto_focus;
#[cfg(feature = "livestream")]
pub mod livestream;
pub mod mirror;
pub mod python;
pub mod qr_code;
pub mod thermal;

use agentwire::agent::Process as _;
use eyre::Result;
use nix::fcntl::{fcntl, FcntlArg, FdFlag};
use std::{
    error::Error,
    net::UdpSocket,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use crate::logger::{self, create_default_datadog_client};

#[doc(hidden)]
pub const PROCESS_DOGSTATSD_ENV: &str = "ORB_CORE_PROCESS_DOGSTATSD";

/// Takes a process-based agent name, and returns its `call` function.
///
/// # Panics
///
/// If `name` is unknown.
// NOTE: keep track of all process-based agents here!
pub fn call_process_agent(name: &str, fd: OwnedFd) -> Result<(), Box<dyn Error>> {
    logger::init_for_agent();
    match name {
        "mega-agent-one" => python::mega_agent_one::MegaAgentOne::call(fd)?,
        "mega-agent-two" => python::mega_agent_two::MegaAgentTwo::call(fd)?,
        "rgb-camera-worker" => camera::rgb::worker::Worker::call(fd)?,
        "thermal-camera" => camera::thermal::Sensor::call(fd)?,
        "qr-code" => qr_code::Agent::call(fd)?,
        _ => panic!("unregistered agent {name}"),
    }
    Ok(())
}

struct ProcessInitializer {
    dogstatsd_fd: OwnedFd,
}

impl Default for ProcessInitializer {
    fn default() -> Self {
        let dogstatsd_socket: UdpSocket = create_default_datadog_client().try_into().unwrap();
        let dogstatsd_fd = OwnedFd::from(dogstatsd_socket);
        clear_descriptor_cloexec(&dogstatsd_fd).unwrap();
        Self { dogstatsd_fd }
    }
}

impl agentwire::agent::process::Initializer for ProcessInitializer {
    fn keep_file_descriptors(&self) -> Vec<RawFd> {
        vec![self.dogstatsd_fd.as_raw_fd()]
    }

    fn envs(&self) -> Vec<(String, String)> {
        vec![(PROCESS_DOGSTATSD_ENV.to_string(), self.dogstatsd_fd.as_raw_fd().to_string())]
    }
}

/// Polls the port for commands, finishing when there are no pending commands.
#[macro_export]
macro_rules! poll_commands {
    (|$port:ident, $cx:ident| $($clauses:tt)*) => {
        while let Poll::Ready(command) = Pin::new(&mut *$port).poll_next($cx) {
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
#[cfg(all(not(feature = "no-image-encryption"), any(feature = "internal-data-acquisition", test)))]
fn encrypt_and_seal(plaintext: &[u8]) -> Vec<u8> {
    // encrypt contents
    use sodiumoxide::crypto::sealedbox;
    let ciphertext = sealedbox::seal(plaintext, &crate::consts::WORLDCOIN_ENCRYPTION_PUBKEY);
    assert_ne!(plaintext, ciphertext);
    ciphertext
}

/// Clears CLOEXEC flag on a file descriptor
fn clear_descriptor_cloexec<F: AsRawFd>(fd: &F) -> Result<()> {
    let mut flags = FdFlag::from_bits(fcntl(fd.as_raw_fd(), FcntlArg::F_GETFD)?).unwrap();

    if flags.contains(FdFlag::FD_CLOEXEC) {
        flags.remove(FdFlag::FD_CLOEXEC);
        fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(flags))?;
    }
    Ok(())
}
