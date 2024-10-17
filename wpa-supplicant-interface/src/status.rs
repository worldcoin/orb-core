//! WiFi connection check.

use super::{CONFIG_PATH, DEFAULT_CONFIG_PATH, INTERFACE, WPA_CLI_BIN};
use eyre::{bail, Result, WrapErr};
use std::{fs::copy, process::Command, str};

/// WPA state prefix.
pub const WPA_STATE: &str = "wpa_state=";

/// SSID prefix.
pub const SSID: &str = "ssid=";

/// Runs a stateful Wi-Fi interface + connection check
///
/// Attempts to use `wpa_cli -i <iface> status` to check the status of our
/// network interface ("DISCONNECTED", "COMPLETED", etc.)
pub fn run(field_prefix: &str) -> Result<()> {
    let mut wpa_cli = Command::new(WPA_CLI_BIN);
    wpa_cli.arg("-i").arg(INTERFACE).arg("status");

    let output = wpa_cli.output().wrap_err(format!("spawning `{WPA_CLI_BIN}`"))?;

    if output.status.success() {
        let output = str::from_utf8(&output.stdout)
            .wrap_err(format!("processing `{WPA_CLI_BIN}` output as UTF-8 string"))?;
        let status = parse_output(output, field_prefix)
            .wrap_err(format!("parsing `{WPA_CLI_BIN}` output"))?;

        println!("{status}");
        return Ok(());
    }

    println!("`{WPA_CLI_BIN}` terminated with exit code: {}", output.status);
    println!("stdout: {}", String::from_utf8(output.stdout)?);
    println!("stderr: {}", String::from_utf8(output.stderr)?);
    eprintln!("executing `{WPA_CLI_BIN}` failed on first attempt");
    recover_config()
}

fn parse_output<'a>(output: &'a str, field_prefix: &str) -> Result<&'a str> {
    for line in output.lines() {
        if let Some(status) = line.strip_prefix(field_prefix) {
            return Ok(status);
        }
    }
    bail!("field not found")
}

fn recover_config() -> Result<()> {
    copy(DEFAULT_CONFIG_PATH, CONFIG_PATH)?;

    bail!(
        "Interacting with `{WPA_CLI_BIN}` failed. default wpa_supplicant config has been restored"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disconnected() {
        let output = r"wpa_state=DISCONNECTED
p2p_device_address=ec:63:d7:6f:1b:7b
address=ec:63:d7:6f:1b:7a
uuid=f4418392-5b3f-5d70-bf4b-0eaeecb5ae19
";
        assert_eq!(parse_output(output, WPA_STATE).unwrap(), "DISCONNECTED");
        assert!(parse_output(output, SSID).is_err());
    }

    #[test]
    fn test_connected() {
        let output = r"bssid=dc:2c:6e:14:fc:32
freq=5220
ssid=Worldcoin
id=0
mode=station
pairwise_cipher=CCMP
group_cipher=CCMP
key_mgmt=WPA2-PSK
wpa_state=COMPLETED
ip_address=192.168.2.17
p2p_device_address=ec:63:d7:6f:1b:7b
address=ec:63:d7:6f:1b:7a
uuid=f4418392-5b3f-5d70-bf4b-0eaeecb5ae19
";
        assert_eq!(parse_output(output, WPA_STATE).unwrap(), "COMPLETED");
        assert_eq!(parse_output(output, SSID).unwrap(), "Worldcoin");
    }
}
