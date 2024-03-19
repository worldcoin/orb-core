//! Orb core interface to `wpa_supplicant`.
//!
//! This is a restricted interface for controlling the Orb WiFi networking. It's
//! designed to run with higher privileges than `orb-core` using the Linux
//! SGID/SUID mechanism.

#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

pub mod join;
pub mod reconfigure;
pub mod signal;
pub mod status;

use clap::StructOpt;
use eyre::Result;

/// WiFi interface name.
pub const INTERFACE: &str = "wlan0";

/// Path to `wpa_supplicant` configuration file.
pub const CONFIG_PATH: &str = "/usr/persistent/wpa_supplicant-wlan0.conf";

/// Path to `wpa_supplicant` default backup configuration file.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/default/wpa_supplicant.conf";

// Path to the `wpa_cli` binary
pub const WPA_CLI_BIN: &str = "/sbin/wpa_cli";

/// List of accepted authenticated key management protocols.
pub const ACCEPTED_KEY_MGMT_VALUES: &[&str] =
    &["IEEE8021X", "NONE", "WPA-EAP", "WPA-EAP-SHA256", "WPA-PSK", "WPA-PSK-SHA256"];

#[derive(StructOpt, Debug)]
#[clap(about)]
enum Opt {
    /// Checks the status of WiFi network connection.
    Check,
    /// Joins a new WiFi network.
    Join {
        /// Authenticated key management protocol.
        #[structopt(long)]
        auth: Option<String>,
        /// Network SSID. Only hex string format is accepted.
        #[structopt(long)]
        ssid: String,
        /// Password. Only hex string format is accepted.
        #[structopt(long)]
        password: Option<String>,
    },
    /// Forces wpa_supplicant to re-read its configuration data.
    Reconfigure,
    /// Overwrites the current config file with the default one.
    RestoreDefaultConfig,
    /// Checks the current SSID name.
    Ssid,
    /// Checks the current signal statistics.
    Signal,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    match Opt::parse() {
        Opt::Check => status::run(status::WPA_STATE),
        Opt::Join { auth, ssid, password } => {
            join::run(auth.as_deref(), &ssid, password.as_deref())
        }
        Opt::Reconfigure => reconfigure::run(),
        Opt::RestoreDefaultConfig => {
            std::fs::copy(DEFAULT_CONFIG_PATH, CONFIG_PATH)?;
            Ok(())
        }
        Opt::Ssid => status::run(status::SSID),
        Opt::Signal => signal::run(),
    }
}
