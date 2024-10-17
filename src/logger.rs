//! Logging support.

use crate::agents::PROCESS_DOGSTATSD_ENV;
use dogstatsd::{Client, Options};
use eyre::Result;
use flexi_logger::{
    filter::{LogLineFilter, LogLineWriter},
    style, DeferredNow, Level, Logger, Record,
};
use libc::{isatty, STDOUT_FILENO};
use once_cell::sync::Lazy;
use std::{
    env,
    fmt::Arguments,
    fs::OpenOptions,
    io::Write,
    net::UdpSocket,
    os::fd::FromRawFd,
    path::Path,
    process::Output,
    sync::{atomic::AtomicBool, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime},
};
use time::{format_description, OffsetDateTime};

/// The global suppress flag for datadog metrics.
pub static DATADOG_SUPPRESS: AtomicBool = AtomicBool::new(false);

/// Helper macro to increment a datadog counter.
#[macro_export]
macro_rules! dd_incr {
    (
        $key:literal $(+ format !($str:literal $(, $($arg:tt)*)?))?
        $(, $tag:expr)*
        $(; $tags:expr)?
    ) => {
        if !$crate::logger::DATADOG_SUPPRESS.load(std::sync::atomic::Ordering::Relaxed) {
            #[allow(unused_variables)]
            let tags: &[&str] = &[$($tag),*];
            $(let tags = $tags;)?
            #[allow(unused_variables)]
            let key: &str = concat!("orb.", $key);
            $(let key = &format!(concat!("orb.", $key, ".", $str) $(, $($arg)*)?);)?
            if let Err(err) = $crate::logger::DATADOG.incr(key, tags) {
                ::tracing::error!("Datadog incr reporting failed with error: {err:#?}");
            }
        }
    };
}

/// Helper macro to send a datadog timing metric.
#[macro_export]
macro_rules! dd_timing {
    (
        $key:literal $(+ format !($str:literal $(, $($arg:tt)*)?))?,
        $t:expr
        $(, $tag:expr)*
        $(; $tags:expr)?
    ) => {
        if !$crate::logger::DATADOG_SUPPRESS.load(std::sync::atomic::Ordering::Relaxed) {
            #[allow(unused_variables)]
            let tags: &[&str] = &[$($tag),*];
            $(let tags = $tags;)?
            #[allow(unused_variables)]
            let key: &str = concat!("orb.", $key);
            $(let key = &format!(concat!("orb.", $key, ".", $str) $(, $($arg)*)?);)?
            if let Err(err) =
                $crate::logger::DATADOG.timing(key, $crate::logger::TimeElapsed::elapsed(&$t), tags)
            {
                ::tracing::error!("Datadog timing reporting failed with error: {err:#?}");
            }
        }
    };
}

/// Helper macro to send a datadog gauge metric.
#[macro_export]
macro_rules! dd_gauge {
    (
        $key:literal $(+ format !($str:literal $(, $($arg:tt)*)?))?,
        $value:expr
        $(, $tag:expr)*
        $(; $tags:expr)?
    ) => {
        if !$crate::logger::DATADOG_SUPPRESS.load(std::sync::atomic::Ordering::Relaxed) {
            #[allow(unused_variables)]
            let tags: &[&str] = &[$($tag),*];
            $(let tags = $tags;)?
            #[allow(unused_variables)]
            let key: &str = concat!("orb.", $key);
            $(let key = &format!(concat!("orb.", $key, ".", $str) $(, $($arg)*)?);)?
            if let Err(err) = $crate::logger::DATADOG.gauge(key, $value, tags) {
                ::tracing::error!("Datadog gauge reporting failed with error: {err:#?}");
            }
        }
    };
}

/// Helper macro to send a datadog count metric.
#[macro_export]
macro_rules! dd_count {
    (
        $key:literal $(+ format !($str:literal $(, $($arg:tt)*)?))?,
        $value:expr
        $(, $tag:expr)*
        $(; $tags:expr)?
    ) => {
        if !$crate::logger::DATADOG_SUPPRESS.load(std::sync::atomic::Ordering::Relaxed) {
            #[allow(unused_variables)]
            let tags: &[&str] = &[$($tag),*];
            $(let tags = $tags;)?
            #[allow(unused_variables)]
            let key: &str = concat!("orb.", $key);
            $(let key = &format!(concat!("orb.", $key, ".", $str) $(, $($arg)*)?);)?
            if let Err(err) = $crate::logger::DATADOG.count(key, $value, tags) {
                ::tracing::error!("Datadog count reporting failed with error: {err:#?}");
            }
        }
    };
}

/// Orb identification code.
pub static DATADOG: Lazy<Client> = Lazy::new(init_datadog_client);

fn try_create_datadog_client_from_socket() -> Option<Client> {
    if let Ok(fd) = env::var(PROCESS_DOGSTATSD_ENV) {
        let sock = unsafe { UdpSocket::from_raw_fd(fd.parse().ok()?) };
        return sock.try_into().ok();
    }
    None
}

/// This should only be used before forking a new process-agent. Creates a default datadog client. This default datadog client creates a new FD socket to connect to the actual datadog daemon. This new open socket can be consecutively used by orb-core's process-agents that are inside a network namespace.
#[must_use]
pub fn create_default_datadog_client() -> Client {
    let datadog_options = Options::default();
    Client::new(datadog_options).unwrap()
}

/// We currently have two methods for establishing a connection to the Datadog daemon:
///
/// 1) The main process of orb-core initiates a new client with a UDP socket that connects to the daemon.
///
/// 2) orb-mega-agents run within a network namespace (sandbox), unable to connect to the daemon. They expect a socket passed down from their parent (the main orb-core process). This socket is created in orb-core right before the agent's spawn.
fn init_datadog_client() -> Client {
    try_create_datadog_client_from_socket().unwrap_or_else(create_default_datadog_client)
}

const DEFAULT_LOG_LEVEL: &str = "debug";

struct InternalOnly;
impl LogLineFilter for InternalOnly {
    fn write(
        &self,
        now: &mut DeferredNow,
        record: &Record,
        log_line_writer: &dyn LogLineWriter,
    ) -> std::io::Result<()> {
        // logs with paths that start with "/" are from 3rd party libraries
        if record.file().map_or(true, |file| !file.starts_with('/')) {
            log_line_writer.write(now, record)?;
        }
        Ok(())
    }
}

/// A helper trait to get the elapsed time in milliseconds as an i64.
pub trait TimeElapsed {
    /// Gets the time elapsed in milliseconds as an i64.
    fn elapsed(&self) -> i64;
}

impl TimeElapsed for Instant {
    fn elapsed(&self) -> i64 {
        self.elapsed().as_millis().try_into().unwrap_or(i64::MAX)
    }
}

impl TimeElapsed for SystemTime {
    fn elapsed(&self) -> i64 {
        self.elapsed().unwrap_or(Duration::MAX).as_millis().try_into().unwrap_or(i64::MAX)
    }
}

impl TimeElapsed for Duration {
    fn elapsed(&self) -> i64 {
        self.as_millis().try_into().unwrap_or(i64::MAX)
    }
}

/// Initializes the global logger for the `log` logging facade.
///
/// # Panics
///
/// If logger fails to initialize
pub fn init<const RAW_STDOUT: bool>() {
    static LOGGER: OnceLock<flexi_logger::LoggerHandle> = OnceLock::new();
    // TODO(O-2082): The logger is supposed to be dropped at the end of the program to
    // properly flush. Saving it in a static is not correct. However, the old
    // behavior was to immediately drop, which was not correct either.
    // So this is a compromise till it can be fixed in a later PR, as saving it
    // in a static OnceLock at least prevents double initialization.
    LOGGER.get_or_init(|| {
        Logger::try_with_env_or_str(DEFAULT_LOG_LEVEL)
            .expect("failed to initialize logger")
            .format(format::<RAW_STDOUT>)
            .filter(Box::new(InternalOnly))
            .set_palette("124;3;4;146;7".into())
            .start()
            .expect("failed to initialize the logger")
    });
}

/// Similar to the above, but this initializer should be user for process agents.
///
/// # Panics
///
/// If logger fails to initialize
pub fn init_for_agent() {
    static LOGGER: OnceLock<flexi_logger::LoggerHandle> = OnceLock::new();
    LOGGER.get_or_init(|| {
        Logger::try_with_env_or_str(DEFAULT_LOG_LEVEL)
            .expect("failed to initialize logger")
            .format(agent_format)
            .filter(Box::new(InternalOnly))
            .set_palette("124;3;4;146;7".into())
            .start()
            .expect("failed to initialize the logger")
    });
}

/// Formats a record to match systemd's new-style daemon format.
///
/// This function creates logs that can be ingested by journald by mapping a rust log to an
/// individual record (by removing newlines), and its log level to a syslog/systemd priority.
/// Systemd expects a record to be prefixed with a number `<n>` to indicate its priority, and
/// splits input streams at newline into individual records.
fn format_newstyle_daemon(w: &mut dyn Write, record: &Record<'_>) -> Result<(), std::io::Error> {
    /// Removes newlines and carriages returns, replacing them spaces.
    fn sanitize_args(args: Arguments<'_>) -> String {
        let s = std::fmt::format(args);
        s.trim().replace(['\n', '\r'], " ")
    }
    let priority = match record.level() {
        Level::Error => b"<3>",
        Level::Warn => b"<4>",
        Level::Info => b"<5>",
        Level::Debug => b"<6>",
        Level::Trace => b"<7>",
    };
    w.write_all(priority)?;
    write!(w, "[{}:{}] ", record.file().unwrap_or("<unnamed>"), record.line().unwrap_or(0),)?;
    w.write_all(sanitize_args(*record.args()).as_bytes())?;
    w.write_all(b"\n")?;
    Ok(())
}

fn format<const RAW_STDOUT: bool>(
    w: &mut dyn Write,
    now: &mut DeferredNow,
    record: &Record<'_>,
) -> Result<(), std::io::Error> {
    let tty = unsafe { isatty(STDOUT_FILENO) } != 0;
    if tty {
        let level = record.level();
        let log = format!(
            "[{}] T[{:?}] {: <5} [{}:{}] {}",
            now.now().format("%y-%m-%d %H:%M:%S%.3f %:z"),
            thread::current().name().unwrap_or("<unnamed>"),
            level,
            record.file().unwrap_or("<unnamed>"),
            record.line().unwrap_or(0),
            &record.args()
        );
        write!(w, "{}", style(level).paint(log))?;
        if RAW_STDOUT {
            write!(w, "\r")?;
        }
        Ok(())
    } else {
        format_newstyle_daemon(w, record)
    }
}

fn agent_format(
    w: &mut dyn Write,
    _now: &mut DeferredNow,
    record: &Record<'_>,
) -> Result<(), std::io::Error> {
    let level = record.level();
    let log = format!(
        "{: <5} [{}:{}] {}",
        level,
        record.file().unwrap_or("<unnamed>"),
        record.line().unwrap_or(0),
        &record.args()
    );
    write!(w, "{}", style(level).paint(log))?;
    Ok(())
}

/// Append formatted command output to a log file.
pub fn log_to_file(log_path: &Path, command: &str, output: &Output) -> Result<()> {
    let timestamp = OffsetDateTime::now_utc()
        .format(&format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown time".to_string());

    let message = format!(
        "{} - Error executing '{}': \nSTDOUT:\n{}\nSTDERR:\n{}",
        timestamp,
        command,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut log_file = OpenOptions::new().create(true).append(true).open(log_path)?;
    writeln!(log_file, "{message}")?;
    Ok(())
}
