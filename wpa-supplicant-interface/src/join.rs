//! Join a new WiFi network.

use crate::reconfigure;

use super::{ACCEPTED_KEY_MGMT_VALUES, CONFIG_PATH};
use eyre::{bail, Result, WrapErr};
use std::{fs::File, io::Write, path::Path};

/// Joins a new WiFi network.
pub fn run(auth: Option<&str>, ssid: &str, password: Option<&str>) -> Result<()> {
    if Path::new(CONFIG_PATH).is_symlink() {
        bail!("Unexpected symbolic link in place of {}", CONFIG_PATH);
    }
    {
        let mut file = File::options()
            .write(true)
            .truncate(true)
            .create(true)
            .open(CONFIG_PATH)
            .wrap_err("opening wpa_supplicant config")?;
        render_conf(&mut file, auth, ssid, password).wrap_err("writing wpa_supplicant config")?;
    }
    reconfigure::run()?;
    Ok(())
}

fn render_conf<W: Write>(
    w: &mut W,
    auth: Option<&str>,
    ssid: &str,
    password: Option<&str>,
) -> Result<()> {
    writeln!(w, "ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=netdev")?;
    writeln!(w)?;
    writeln!(w, "network={{")?;
    if let Some(auth) = auth {
        if !ACCEPTED_KEY_MGMT_VALUES.contains(&auth) {
            bail!("Invalid auth protocol {:?}", auth);
        }
        writeln!(w, "    key_mgmt={auth}")?;
    }
    check_hex_string_format(ssid).wrap_err("setting ssid field")?;
    writeln!(w, "    ssid={ssid}")?;
    if let Some(password) = password {
        check_hex_string_format(password).wrap_err("setting psk field")?;
        writeln!(w, "    psk={password}")?;
    }
    writeln!(w, "}}")?;
    Ok(())
}

fn check_hex_string_format(string: &str) -> Result<()> {
    if string.len() % 2 == 0 && string.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        bail!("Invalid hex string input: {:?}", string);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn test_full() {
        let mut output = Cursor::new(Vec::new());
        render_conf(&mut output, Some("WPA-PSK"), "001122", Some("334455")).unwrap();
        output.set_position(0);
        let mut string = String::new();
        output.read_to_string(&mut string).unwrap();
        assert_eq!(
            string,
            r"ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=netdev

network={
    key_mgmt=WPA-PSK
    ssid=001122
    psk=334455
}
"
        );
    }

    #[test]
    fn test_only_ssid() {
        let mut output = Cursor::new(Vec::new());
        render_conf(&mut output, None, "001122", None).unwrap();
        output.set_position(0);
        let mut string = String::new();
        output.read_to_string(&mut string).unwrap();
        assert_eq!(
            string,
            r"ctrl_interface=DIR=/var/run/wpa_supplicant GROUP=netdev

network={
    ssid=001122
}
"
        );
    }

    #[test]
    fn test_invalid_hex_string() {
        let mut output = Cursor::new(Vec::new());
        let res = render_conf(&mut output, Some("WPA-PSK"), "non-hex-string", None);
        assert!(res.is_err());
    }

    #[test]
    fn test_invalid_auth() {
        let mut output = Cursor::new(Vec::new());
        let res = render_conf(&mut output, Some("invalid-auth"), "non-hex-string", None);
        assert!(res.is_err());
    }
}
