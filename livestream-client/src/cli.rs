//! Command Line Interface.

use std::net::IpAddr;

use clap::StructOpt;

/// Orb Livestream Client
#[derive(StructOpt, Debug)]
#[clap(about, version)]
pub struct Cli {
    /// IP-address of the Orb
    pub ip: IpAddr,
}
