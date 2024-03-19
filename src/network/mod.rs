//! Orb networking.

pub mod mecard;

use self::mecard::{AuthType, Credentials};
use crate::monitor::net::ping;
use data_encoding::HEXLOWER;
use eyre::{eyre, Result, WrapErr};
use ring::{pbkdf2, pbkdf2::PBKDF2_HMAC_SHA1};
use std::{num::NonZeroU32, process::Command, str};
use tokio::task::spawn_blocking;

const WPA_SUPPLICANT_INTERFACE_BIN: &str = "/usr/local/bin/wpa-supplicant-interface";

/// Network connection status.
#[derive(Debug, Clone, Copy)]
pub enum Status {
    /// WiFi is connected.
    Connected {
        /// True when there is connection to the backend
        has_internet: bool,
    },
    /// WiFi is disconnected.
    Disconnected,
    /// Connection is in progress.
    InProgress,
}

/// Checks WiFi network connection. Returns `true` if connected to a
/// network. The internet connection is not checked.
pub async fn status() -> Result<Status> {
    spawn_blocking(|| {
        if ping().is_some() {
            tracing::info!("Found working connection to the backend. Skipping WiFi status check.");
            return Ok(Status::Connected { has_internet: true });
        }
        tracing::info!("No connection to the backend. Checking WiFi status.");
        let output = Command::new(WPA_SUPPLICANT_INTERFACE_BIN)
            .arg("check")
            .output()
            .wrap_err("running `wpa-supplicant-interface`")?;
        if output.status.success() {
            let output = str::from_utf8(&output.stdout)
                .wrap_err("parsing `wpa-supplicant-interface` output")?
                .trim();
            // All possible variants according to wpa_supplicant source code.
            #[allow(clippy::wildcard_in_or_patterns)]
            match output {
                "COMPLETED" => {
                    if unsafe { libc::res_init() } != 0 {
                        tracing::error!("Failed to re-initialize DNS resolver");
                    }
                    let has_internet = ping().is_some();
                    Ok(Status::Connected { has_internet })
                }
                "DISCONNECTED" | "INACTIVE" | "INTERFACE_DISABLED" => Ok(Status::Disconnected),
                "SCANNING" | "AUTHENTICATING" | "ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE"
                | "GROUP_HANDSHAKE" | "UNKNOWN" | _ => Ok(Status::InProgress),
            }
        } else {
            tracing::warn!("`wpa-supplicant-interface` terminated unsuccessfully");
            tracing::debug!(
                "`wpa-supplicant-interface` terminated with status code `{:?}` and stderr `{:?}`",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(Status::InProgress)
        }
    })
    .await?
}

/// Joins WiFi network using the given `credentials`.
pub async fn join(credentials: Credentials) -> Result<()> {
    spawn_blocking(move || {
        let mut cmd = Command::new(WPA_SUPPLICANT_INTERFACE_BIN);
        cmd.arg("join");
        match credentials.auth_type {
            AuthType::Wep => {
                cmd.arg("--auth").arg("NONE");
            }
            AuthType::Wpa | AuthType::Sae => {
                cmd.arg("--auth").arg("WPA-PSK");
            }
            AuthType::Nopass => {}
        }
        cmd.arg("--ssid").arg(hex_string(credentials.ssid.as_bytes()));
        if let Some(password) = &credentials.password {
            cmd.arg("--password").arg(wpa_passphrase(&credentials.ssid, password));
        }
        cmd.status()
            .wrap_err("running `wpa-supplicant-interface`")?
            .success()
            .then_some(())
            .ok_or_else(|| eyre!("`wpa-supplicant-interface` terminated unsuccessfully"))?;
        Ok(())
    })
    .await?
}

/// Restores the default wpa_supplicant.conf configuration file.
/// Then forces wpa_supplicant to re-read its configuration file,
/// thus disconnecting from the current network.
pub async fn reset() -> Result<()> {
    spawn_blocking(move || {
        Command::new(WPA_SUPPLICANT_INTERFACE_BIN)
            .arg("restore-default-config")
            .status()
            .wrap_err("running `wpa-supplicant-interface`")?
            .success()
            .then_some(())
            .ok_or_else(|| eyre!("`wpa-supplicant-interface` terminated unsuccessfully"))?;

        Command::new(WPA_SUPPLICANT_INTERFACE_BIN)
            .arg("reconfigure")
            .status()
            .wrap_err("running `wpa-supplicant-interface`")?
            .success()
            .then_some(())
            .ok_or_else(|| eyre!("`wpa-supplicant-interface` terminated unsuccessfully"))
    })
    .await?
}

// Using hex string encoding, because `wpa_supplicant.conf` string escaping
// schema is not well-defined.
fn hex_string<T: AsRef<[u8]>>(input: T) -> String {
    HEXLOWER.encode(input.as_ref())
}

fn wpa_passphrase(ssid: &str, passphrase: &str) -> String {
    let mut hash = [0_u8; 32];
    pbkdf2::derive(
        PBKDF2_HMAC_SHA1,
        NonZeroU32::new(4096).unwrap(),
        ssid.as_bytes(),
        passphrase.as_bytes(),
        &mut hash,
    );
    hex_string(hash)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_hex_string() {
        assert_eq!(hex_string(b"worldcoin"), "776f726c64636f696e");
        assert_eq!(hex_string(b"\0"), "00");
    }

    #[test]
    fn test_wpa_passphrase() {
        assert_eq!(
            wpa_passphrase("worldcoin", "12345678"),
            "5c1f986129b5a10564a66899f10a2989d4deb8f9a9ba504c68e535d7a3c8e5ba"
        );
    }
}
