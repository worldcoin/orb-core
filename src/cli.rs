//! Command Line Interface.

#[cfg(feature = "integration_testing")]
use crate::plans;
use clap::StructOpt;
use std::path::PathBuf;

/// The rust main program running on the orb and responsible for signup and
/// other main behaviors of the orb
#[allow(clippy::struct_excessive_bools)]
#[derive(StructOpt, Debug)]
#[clap(about, version = env!("GIT_VERSION"))]
pub struct Cli {
    /// Enable livestream
    #[cfg(feature = "livestream")]
    #[structopt(short = 'l', long)]
    pub livestream: bool,
    /// Provide a custom operator QR code. If an empty string is provided, then
    /// we use an internal testing operator code.
    #[structopt(short = 'o', long)]
    pub operator_qr_code: Option<Option<String>>,
    /// Provide a custom user QR code. If an empty string is provided, then we
    /// use an internal testing operator code.
    #[structopt(short = 'u', long)]
    pub user_qr_code: Option<Option<String>>,
    /// Exit after the first successful signup.
    #[structopt(short = 'O', long)]
    pub oneshot: bool,
    /// Skip biometric pipeline.
    #[cfg(feature = "allow-plan-mods")]
    #[structopt(short = 'P', long)]
    pub skip_pipeline: bool,
    /// Skip fraud checks.
    #[cfg(feature = "allow-plan-mods")]
    #[structopt(short = 'F', long)]
    pub skip_fraud_checks: bool,
    /// Provide a path to a directory with biometric data instead of running
    /// biometric capture. Implies `--oneshot`.
    #[cfg(feature = "allow-plan-mods")]
    #[structopt(short = 'b', long)]
    pub biometric_input: Option<PathBuf>,
    /// Ignore missing sounds instead of panicking.
    #[structopt(long)]
    pub ignore_missing_sounds: bool,
    /// Various hacks for a signup to pass on hon-human-subjects
    #[cfg(feature = "integration_testing")]
    #[structopt(long)]
    pub ci_hacks: Option<plans::integration_testing::CiHacks>,
    /// Enable fetching auth token for backend communication, only planned to be used in control API.
    #[structopt(short = 't', long)]
    pub enable_auth_token: bool,
    /// Load config from file.
    #[structopt(short = 'c', long)]
    pub config: Option<PathBuf>,
    /// Enable data acquisition mode.
    #[cfg(feature = "internal-data-acquisition")]
    #[structopt(short = 'd', long)]
    pub data_acquisition: bool,
}
