//! Force wpa_supplicant to re-read its configuration data.

use crate::{INTERFACE, WPA_CLI_BIN};
use eyre::{eyre, Context, Result};
use std::process::Command;

/// Force wpa_supplicant to re-read its configuration data.
pub fn run() -> Result<()> {
    Command::new(WPA_CLI_BIN)
        .arg("-i")
        .arg(INTERFACE)
        .arg("reconfigure")
        .status()
        .wrap_err(format!("running `{WPA_CLI_BIN}`"))?
        .success()
        .then_some(())
        .ok_or_else(|| eyre!("`{WPA_CLI_BIN}` terminated unsuccessfully"))
}
