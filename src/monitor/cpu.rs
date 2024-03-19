//! CPU monitor.

use crate::utils::spawn_named_thread;
use eyre::{bail, eyre, Result};
use futures::prelude::*;
use std::{
    collections::VecDeque,
    fs,
    pin::Pin,
    task::{ready, Context, Poll},
    thread::sleep,
    time::Duration,
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};

const REPORT_INTERVAL: Duration = Duration::from_millis(500);
const REPORT_LENGTH: usize = 6; // 3000ms report window
const REPORT_CHANNEL_CAPACITY: usize = 10;

/// CPU monitor trait.
pub trait Monitor: Stream<Item = Report> + Send + Unpin {
    /// Returns a new handler to the shared CPU monitor interface.
    fn clone(&self) -> Box<dyn Monitor>;

    /// Returns the latest CPU monitor report.
    fn last_report(&mut self) -> Result<Option<&Report>>;
}

/// CPU monitor for the Orb hardware.
pub struct Jetson {
    report_tx: broadcast::Sender<Report>,
    report_rx: BroadcastStream<Report>,
    last_report: Option<Report>,
}

/// CPU monitor which does nothing.
pub struct Fake;

/// Periodic CPU monitor report.
#[derive(Clone, Debug)]
pub struct Report {
    /// Fraction of time spent in all other modes than idle.
    pub cpu_load: f64,
}

#[allow(dead_code)]
struct Stat {
    /// Time spent in user mode.
    user: u64,
    /// Time spent in user mode with low priority (nice).
    nice: u64,
    /// Time spent in system mode.
    system: u64,
    /// Time spent in the idle task. This value should be USER_HZ times the second entry in the /proc/uptime pseudo-file.
    idle: u64,
    /// Time waiting for I/O to complete.
    iowait: u64,
    /// Time servicing interrupts.
    irq: u64,
    /// Time servicing softirqs.
    softirq: u64,
    /// Stolen time, which is the time spent in other operating systems when running in a virtualized environment
    steal: u64,
    /// Time spent running a virtual CPU for guest operating systems under the control of the Linux kernel.
    guest: u64,
    /// Time spent running a niced guest (virtual CPU for guest operating systems under the control of the Linux kernel).
    guest_nice: u64,
    /// Sum of all of the fields.
    total: u64,
}

impl Monitor for Fake {
    fn clone(&self) -> Box<dyn Monitor> {
        Box::new(Self)
    }

    fn last_report(&mut self) -> Result<Option<&Report>> {
        Ok(None)
    }
}

impl Stream for Fake {
    type Item = Report;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

impl Monitor for Jetson {
    fn clone(&self) -> Box<dyn Monitor> {
        Box::new(Self {
            report_tx: self.report_tx.clone(),
            report_rx: BroadcastStream::new(self.report_tx.subscribe()),
            last_report: None,
        })
    }

    /// Returns the latest CPU monitor report.
    fn last_report(&mut self) -> Result<Option<&Report>> {
        while let Some(report) = self.next().now_or_never() {
            if let Some(report) = report {
                self.last_report = Some(report);
            } else {
                bail!("broadcast channel of CPU monitor exited");
            }
        }
        Ok(self.last_report.as_ref())
    }
}

impl Stream for Jetson {
    type Item = Report;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            break Poll::Ready(match ready!(self.report_rx.poll_next_unpin(cx)) {
                Some(Ok(item)) => Some(item),
                Some(Err(BroadcastStreamRecvError::Lagged(_))) => continue,
                None => None,
            });
        }
    }
}

impl Jetson {
    /// Spawns a new CPU monitor.
    #[must_use]
    pub fn spawn() -> Self {
        let (report_tx, report_rx) = broadcast::channel(REPORT_CHANNEL_CAPACITY);
        let report_tx2 = report_tx.clone();
        spawn_named_thread("monitor-cpu", move || match main_loop(&report_tx2) {
            Ok(()) => tracing::warn!("Network monitor main loop exited"),
            Err(err) => tracing::error!("Network monitor main loop error: {err}"),
        });
        Self { report_tx, report_rx: BroadcastStream::new(report_rx), last_report: None }
    }
}

impl Report {
    #[allow(clippy::cast_precision_loss)]
    fn from_stat(prev_stat: &Stat, next_stat: &Stat) -> Self {
        let idle_delta = next_stat.idle - prev_stat.idle;
        let total_delta = next_stat.total - prev_stat.total;
        let cpu_load = 1.0 - idle_delta as f64 / total_delta as f64;
        Self { cpu_load }
    }
}

fn main_loop(report_tx: &broadcast::Sender<Report>) -> Result<()> {
    let mut stats = VecDeque::new();
    loop {
        let mut values = fs::read_to_string("/proc/stat")?
            .lines()
            .find(|line| line.starts_with("cpu "))
            .ok_or_else(|| eyre!("unknown /proc/stat format"))?
            .split_whitespace()
            .skip(1)
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()?;
        let total = values.iter().sum();
        values.truncate(10);
        let values =
            <[u64; 10]>::try_from(values).map_err(|_| eyre!("unknown /proc/stat format"))?;
        let [user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice] = values;
        let next_stat = Stat {
            user,
            nice,
            system,
            idle,
            iowait,
            irq,
            softirq,
            steal,
            guest,
            guest_nice,
            total,
        };
        if let Some(prev_stat) = stats.back() {
            report_tx.send(Report::from_stat(prev_stat, &next_stat))?;
        }
        if stats.len() == REPORT_LENGTH {
            stats.pop_back();
        }
        stats.push_front(next_stat);
        sleep(REPORT_INTERVAL);
    }
}
