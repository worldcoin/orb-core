//! The rust main program running on the orb and responsible for signup and
//! other main behaviors of the orb.
//!
//! # Architecture
//!
//! See the [`brokers`] module documentation for the description of the chosen
//! architecture.
//!
//! # Guidelines
//!
//! The code should be formatted with Rustfmt using the project-level
//! `rustfmt.toml`. E.g. run from the command line: `cargo fmt`.
//!
//! The code should pass clippy lints in pedantic mode. E.g. run from the
//! command line: `cargo clippy`. It's fine to suppress some lint locally with
//! `#[allow(clippy:<lint>)]` attribute.
//!
//! The code should be properly documented and should pass the
//! `#[warn(missing_docs)]` lint.
//!
//! The code should pass the official [Rust API
//! Guidelines](https://rust-lang.github.io/api-guidelines/checklist.html)

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc, clippy::missing_panics_doc)]

pub mod agents;
pub mod backend;
pub mod brokers;
pub mod calibration;
pub mod cli;
pub mod config;
pub mod consts;
pub mod dbus;
pub mod debug_report;
pub mod dsp;
pub mod ext;
pub mod identification;
pub mod image;
pub mod logger;
pub mod mcu;
pub mod monitor;
pub mod network;
pub mod pid;
pub mod plans;
pub mod process;
pub mod secure_element;
pub mod short_lived_token;
pub mod ssd;
pub mod time_series;
pub mod timestamped;
pub mod ui;
pub mod utils;
pub mod versions_json;

use agents::call_process_agent;
use agentwire::agent;
use eyre::Result;
use futures::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A wrapper for the main function, which runs common initialization routines
/// and takes a future to execute as the main function.
#[allow(clippy::missing_panics_doc)]
pub fn async_main<F: Future<Output = Result<()>>>(f: F) -> Result<()> {
    std::env::set_var("LD_LIBRARY_PATH", "/usr/lib/aarch64-linux-gnu/gstreamer-1.0");
    std::env::set_var("LD_PRELOAD", "/usr/lib/aarch64-linux-gnu/libGLdispatch.so");
    color_eyre::install()?;
    gstreamer::init()?;
    std::env::remove_var("LD_LIBRARY_PATH");
    std::env::remove_var("LD_PRELOAD");
    agent::process::init(call_process_agent);
    let future = async {
        let result = f.await;
        match result {
            Ok(()) => {
                // If we return from this function, other async tasks in this tokio
                // runtime will keep running. We are completely done by now, it's
                // safe to forcefully kill them.
                std::process::exit(0);
            }
            Err(err) => {
                tracing::error!("Fatal error: {err:?}");
                std::process::exit(1);
            }
        }
    };
    tokio::runtime::Builder::new_multi_thread()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::Relaxed);
            format!("orb-core-worker-{id}")
        })
        .enable_all()
        .build()
        .expect("failed to initialize async runtime")
        .block_on(future)
}
