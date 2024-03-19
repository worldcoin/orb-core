//! Operator QR-code scanning.

use super::{user, Schema};
use crate::{led, sound, sound::Voice};
use once_cell::sync::Lazy;
use regex::Regex;

/// An opt-in operator qr code for testing purposes.
pub const DUMMY_OPERATOR_QR_CODE: &str = "userid:66ad4897-0ca7-4727-8365-ca808348e3cd:1";

static MAGIC_QR_CODE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
        ^
        magic_action
        :
        (?P<magic_action>[\w]+)
        $
    ",
    )
    .expect("bad regex")
});

/// Operator QR-code data.
#[derive(Clone, Debug)]
pub enum Data {
    /// Normal Operator QR-code.
    Normal(user::Data),
    /// Action to reconfigure WiFi.
    MagicResetWifi,
    /// Action to reset mirror calibration.
    MagicResetMirror,
}

impl Schema for Data {
    fn sound() -> sound::Type {
        sound::Type::Voice(Voice::Silence)
    }

    fn led() -> led::QrScanSchema {
        led::QrScanSchema::Operator
    }

    fn try_parse(code: &str) -> Option<Self> {
        let normal = user::Data::try_parse(code).map(Data::Normal);
        if normal.is_some() {
            return normal;
        }
        if let Some(captures) = MAGIC_QR_CODE.captures(code) {
            return match captures
                .name("magic_action")
                .expect("magic_action group must be present")
                .as_str()
            {
                "reset_wifi_credentials" => Some(Data::MagicResetWifi),
                "reset_mirror_calibration" => Some(Data::MagicResetMirror),
                _ => None,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qr_code_variants() {
        {
            let code = "userid:66ad4897-0ca7-4727-8365-ca808348e3cd:1";
            assert!(matches!(Data::try_parse(code), Some(Data::Normal(_))));
        }
        {
            let code = "magic_action:reset_wifi_credentials";
            assert!(matches!(Data::try_parse(code), Some(Data::MagicResetWifi)));
        }
        {
            let code = "magic_action:reset_mirror_calibration";
            assert!(matches!(Data::try_parse(code), Some(Data::MagicResetMirror)));
        }
        {
            let code = "magic_action:burn_and_destroy_everything";
            assert!(Data::try_parse(code).is_none());
        }
        {
            let code = "random_text";
            assert!(Data::try_parse(code).is_none());
        }
    }
}
