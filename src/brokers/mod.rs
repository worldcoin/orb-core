//! Collection of brokers.

mod observer;
mod orb;

pub use self::{
    observer::{
        Builder as ObserverBuilder, DefaultPlan as DefaultObserverPlan, Observer,
        Plan as ObserverPlan,
    },
    orb::{Builder, Orb, Plan as OrbPlan, StateRx as OrbStateRx},
};

use futures::{
    future::{BoxFuture, Either},
    prelude::*,
};
use std::pin::pin;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{ChildStderr, ChildStdout},
};

/// Creates a new process agent logger builder.
pub fn process_logger(
    enable_pruning: bool,
) -> impl Fn(&'static str, ChildStdout, ChildStderr) -> BoxFuture<()> + Send + 'static {
    move |agent_name, stdout, stderr| {
        Box::pin(async move {
            let mut stdout = BufReader::new(stdout).lines();
            let mut stderr = BufReader::new(stderr).lines();
            let mut last_stdout_line = String::new();
            let mut last_stderr_line = String::new();
            loop {
                match future::select(pin!(stdout.next_line()), pin!(stderr.next_line())).await {
                    Either::Left((Ok(Some(line)), _)) => {
                        if enable_pruning && (line.is_empty() || last_stdout_line == line) {
                            continue;
                        }
                        tracing::info!("[{agent_name}] <STDOUT> {line}");
                        last_stdout_line = line;
                    }
                    Either::Right((Ok(Some(line)), _)) => {
                        if enable_pruning && (line.is_empty() || last_stderr_line == line) {
                            continue;
                        }
                        tracing::info!("[{agent_name}] <STDERR> {line}");
                        last_stderr_line = line;
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
