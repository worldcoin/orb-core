//! User QR-code scanning.

use super::Schema;
use crate::{led, sound, sound::Voice};
use once_cell::sync::Lazy;
use orb_qr_link::{decode_qr, DataPolicy};
use regex::{Captures, Regex};

/// An opt-in user qr code for testing purposes.
pub const DUMMY_USER_QR_CODE: &str = "userid:cf37084e-5087-484c-b5a3-3ca3c34016d1:1";

static QR_CODE_V2: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
            ^
            userid
            :
            (?P<user_id>
                [a-z0-9]{8}-
                [a-z0-9]{4}-
                [a-z0-9]{4}
                (?:
                    -
                    [a-z0-9]{4}-
                    [a-z0-9]{12}
                )?
            )
            :
            (?P<data_policy>\d{1,10})
            $
        ",
    )
    .expect("bad regex")
});

/// User QR-code data.
#[derive(Default, Clone, Debug)]
pub struct Data {
    /// User ID in format of 128-bit UUIDv4.
    pub user_id: String,
    /// Data policy.
    pub data_policy: Option<DataPolicy>,
    /// Hash of the user data stored in the backend.
    pub user_data_hash: Option<Vec<u8>>,
}

impl Schema for Data {
    fn sound() -> sound::Type {
        sound::Type::Voice(Voice::Silence)
    }

    fn led() -> led::QrScanSchema {
        led::QrScanSchema::User
    }

    fn try_parse(code: &str) -> Option<Self> {
        if let Ok((user_id, user_data_hash)) = decode_qr(code) {
            return Some(Self {
                user_id: user_id.hyphenated().to_string(),
                data_policy: None,
                user_data_hash: Some(user_data_hash),
            });
        }
        if let Some(captures) = QR_CODE_V2.captures(code) {
            return Some(Data::from_v2(&captures));
        }
        None
    }
}

impl Data {
    #[must_use]
    fn from_v2(captures: &Captures) -> Self {
        let user_id = captures
            .name("user_id")
            .expect("user_id capture group must be present")
            .as_str()
            .to_string();
        let data_policy = captures
            .name("data_policy")
            .expect("data_policy capture group must be present")
            .as_str()
            .parse::<u32>()
            .ok()
            .and_then(|flag| match flag {
                1 => Some(DataPolicy::FullDataOptIn),
                _ => None,
            })
            .unwrap_or_default();
        Self { user_id, data_policy: Some(data_policy), user_data_hash: None }
    }
}

#[cfg(test)]
mod tests {
    use crate::plans::qr_scan::operator::DUMMY_OPERATOR_QR_CODE;

    use super::*;

    #[test]
    fn test_cli_operator() {
        let data = Data::from_v2(&QR_CODE_V2.captures(DUMMY_OPERATOR_QR_CODE).unwrap());
        assert_eq!(data.user_id, "66ad4897-0ca7-4727-8365-ca808348e3cd");
        assert_eq!(data.data_policy.unwrap(), DataPolicy::FullDataOptIn);
    }

    #[test]
    fn test_cli_user() {
        let data = Data::from_v2(&QR_CODE_V2.captures(DUMMY_USER_QR_CODE).unwrap());
        assert_eq!(data.user_id, "cf37084e-5087-484c-b5a3-3ca3c34016d1");
        assert_eq!(data.data_policy.unwrap(), DataPolicy::FullDataOptIn);
    }

    #[test]
    fn test_v2() {
        let text = "userid:3bcf883d-ce22-4a03-8608-4a8a01b88d4d:1";
        let data = Data::from_v2(&QR_CODE_V2.captures(text).unwrap());
        assert_eq!(data.user_id, "3bcf883d-ce22-4a03-8608-4a8a01b88d4d");
        assert!(matches!(data.data_policy.unwrap(), DataPolicy::FullDataOptIn));
    }

    #[test]
    fn test_v2_shortened() {
        let text = "userid:3bcf883d-ce22-4a03:1";
        let data = Data::from_v2(&QR_CODE_V2.captures(text).unwrap());
        assert_eq!(data.user_id, "3bcf883d-ce22-4a03");
        assert!(matches!(data.data_policy.unwrap(), DataPolicy::FullDataOptIn));
    }
}
