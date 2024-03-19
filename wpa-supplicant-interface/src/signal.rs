//! WiFi connection signal check.

use super::{INTERFACE, WPA_CLI_BIN};
use eyre::{bail, eyre, Result, WrapErr};
use std::{process::Command, str};

/// Runs WiFi connection signal check.
pub fn run() -> Result<()> {
    let mut cmd = Command::new(WPA_CLI_BIN);
    cmd.arg("-i").arg(INTERFACE);
    cmd.arg("signal_poll");
    let output = cmd.output().wrap_err(format!("running `{WPA_CLI_BIN}`"))?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| eyre!("`{WPA_CLI_BIN}` terminated unsuccessfully"))?;
    let output =
        str::from_utf8(&output.stdout).wrap_err(format!("parsing `{WPA_CLI_BIN}` output"))?;
    let rssi = parse_output(output).wrap_err(format!("parsing `{WPA_CLI_BIN}` output"))?;
    print!("{rssi}");
    Ok(())
}

fn parse_output(output: &str) -> Result<i64> {
    for line in output.lines() {
        if let Some(rssi) = line.strip_prefix("RSSI=") {
            return Ok(rssi.parse()?);
        }
    }
    bail!("RSSI value not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rssi() {
        let output = r"RSSI=-54
LINKSPEED=433
NOISE=9999
FREQUENCY=5500
WIDTH=80 MHz
AVG_RSSI=60
AVG_BEACON_RSSI=-60
";
        assert_eq!(parse_output(output).unwrap(), -54);
    }
}
