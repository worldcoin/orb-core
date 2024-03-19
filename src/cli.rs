//! Command Line Interface.

use clap::StructOpt;

/// Command linke options.
#[allow(clippy::struct_excessive_bools)]
#[derive(StructOpt, Debug)]
#[clap(about, version = env!("GIT_VERSION"))]
pub struct Cli {
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
    /// Ignore missing sounds instead of panicking.
    #[structopt(long)]
    pub ignore_missing_sounds: bool,
}
