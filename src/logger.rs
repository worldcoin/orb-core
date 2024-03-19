//! Logging support.

use dogstatsd::{Client, DogstatsdResult, Options};
use flexi_logger::{
    filter::{LogLineFilter, LogLineWriter},
    style, DeferredNow, Level, Logger, Record,
};
use libc::{isatty, STDOUT_FILENO};
use once_cell::sync::Lazy;
use std::{fmt::Arguments, io::prelude::*, sync::OnceLock, thread};

/// Orb identification code.
pub static DATADOG: Lazy<Client> = Lazy::new(init_datadog_client);

/// Removes the need to put` &[] as &[&str]` everywhere.
pub const NO_TAGS: &[&str] = &[];

fn init_datadog_client() -> Client {
    let datadog_options = Options::default();
    Client::new(datadog_options).unwrap()
}

/// Helper macro to get the elapsed time in milliseconds and as an i64 from SystemTime.
/// In case of error, it defaults to `i64::MAX`.
macro_rules! sys_elapsed {
    ($e:expr) => {
        $e.elapsed().unwrap_or(std::time::Duration::MAX).as_millis().try_into().unwrap_or(i64::MAX)
    };
}
pub(crate) use sys_elapsed;

/// Helper macro to get the elapsed time in milliseconds and as an i64 from Instant.
/// In case of error, it defaults to `i64::MAX`.
macro_rules! inst_elapsed {
    ($e:expr) => {
        $e.elapsed().as_millis().try_into().unwrap_or(i64::MAX)
    };
}
pub(crate) use inst_elapsed;

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

/// A trait for logging errors instead of propagating the error with `?`.
pub trait LogOnError {
    /// Logs an error message to the default logger at the `Error` level.
    fn or_log(&self);
}

impl LogOnError for DogstatsdResult {
    fn or_log(&self) {
        if let Err(e) = self {
            tracing::error!("Datadog reporting failed with error: {e:#?}");
        }
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
