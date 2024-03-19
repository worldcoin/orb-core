//! WiFi connection check.

use super::{CONFIG_PATH, DEFAULT_CONFIG_PATH, INTERFACE, WPA_CLI_BIN};
use eyre::{bail, Result, WrapErr};
use std::{fs::copy, process::Command, str, thread, time::Duration};

/// WPA state prefix.
pub const WPA_STATE: &str = "wpa_state=";

/// SSID prefix.
pub const SSID: &str = "ssid=";

/// Runs a stateful Wi-Fi interface + connection check
///
/// Attempts to use `wpa_cli -i <iface> status` to check the status of our
/// network interface ("DISCONNECTED", "COMPLETED", etc.)
///
/// The `wpa_cli` command can fail when the interface itself fails to come up
/// (the reason for the `ifup@<interface>.service` failing isn't known at this time.)
/// When we hit this state, we can reliably resolve this by restarting the service
/// and then retrying the `wpa_cli` command.
///
/// Once we've ruled out the issue with the known service start failure, we
/// treat the `wpa_cli` failure as a serious issue and attempt to recover
/// networking by overwriting the `wpa_supplicant.conf` with one from `/etc/default`
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

    eprintln!("executing `{WPA_CLI_BIN}` failed on first attempt");
    eprintln!("encountered known issue with `ifup@{INTERFACE}.service` failing");
    eprintln!("attempt to bring up `ifup@{INTERFACE}.service` by ourselves");

    let output = Command::new("/bin/systemctl")
        .arg("restart")
        .arg(format!("ifup@{INTERFACE}.service"))
        .output()
        .wrap_err(format!("spawning `systemctl restart ifup@{INTERFACE}.service`"))?;

    if !output.status.success() {
        eprintln!(
            "command `/bin/systemctl restart ifup@{INTERFACE}.service` failed with status code \
             `{:?}` and stderr `{:?}`",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("continuing anyway");
    }

    eprintln!("sleeping 10 seconds to give `ifup@{INTERFACE}.service` a chance to come up");
    thread::sleep(Duration::from_secs(10));

    let output = wpa_cli.output().wrap_err(format!("spawning `{WPA_CLI_BIN}` (attempt 2)"))?;

    if output.status.success() {
        let output = str::from_utf8(&output.stdout)
            .wrap_err(format!("processing `{WPA_CLI_BIN}` output as UTF-8 string"))?;
        let status = parse_output(output, field_prefix)
            .wrap_err(format!("parsing `{WPA_CLI_BIN}` output"))?;

        println!("{status}");
        return Ok(());
    }

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

    let mut cmd = Command::new("/bin/systemctl");
    cmd.arg("restart");
    cmd.arg(format!("ifup@{INTERFACE}.service"));
    let output =
        cmd.output().wrap_err_with(|| format!("trying restart `ifup@{INTERFACE}.service`"))?;
    if !output.status.success() {
        eprintln!("failed to bring up interface `ifup@{INTERFACE}.service`");
    }
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
