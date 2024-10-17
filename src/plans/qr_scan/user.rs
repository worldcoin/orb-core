//! User QR-code scanning.

use super::Schema;
use crate::ui;
use once_cell::sync::Lazy;
use orb_qr_link::decode_qr;
use regex::{Captures, Regex};
#[cfg(feature = "internal-data-acquisition")]
use std::fmt;

/// Generated QR code from session "6943c6d9-48bf-4f29-9b60-48c63222e3ea".
pub const DUMMY_USER_QR_CODE: &str = "3aUPG2Ui/TymbYEjGMiLj6q4Dy1S8KnShj27PD/RCANo";

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

#[cfg(any(feature = "internal-data-acquisition", test))]
static QR_CODE_SIGNUP_EXTENSION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
            ^
            userid
            :
            (?P<user_id>
                [a-z0-9_-]+
            )
            :
            (?P<data_policy>\d{1,10})
            (::
                (?P<mode>[a-z0-9]+)
                (:
                    (?P<parameters>[a-z0-9:]+)
                )?
            )?
            ::$
        ",
    )
    .expect("bad regex")
});

/// Data acquisition configuration.
#[derive(Clone, Debug)]
pub struct SignupExtensionConfig {
    /// Data acquisition mode specifier - data-acqusition, focus-sweep, etc.
    pub mode: SignupMode,
    /// Optional data acquisition parameters - multi-wavelength sequence, etc.
    pub parameters: Option<String>,
}

/// User QR-code data.
#[derive(Default, Clone, Debug)]
pub struct Data {
    /// User ID in format of 128-bit UUIDv4.
    pub user_id: String,
    /// It's a data acquisition QR code.
    pub signup_extension: bool,
    /// Data acquisition configuration.
    pub signup_extension_config: Option<SignupExtensionConfig>,
    /// Hash of the user data stored in the backend.
    pub user_data_hash: Option<Vec<u8>>,
}

/// Optional QR-code signup extension modes
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignupMode {
    /// Basic data acquisition mode without extension.
    Basic,
    /// Pupil contraction extension.
    PupilContractionExtension,
    /// Focus sweep extension.
    FocusSweepExtension,
    /// Mirror sweep extension.
    MirrorSweepExtension,
    /// Multi-wavelength extension.
    MultiWavelength,
    /// Overcapture extension.
    Overcapture,
}

impl Schema for Data {
    fn ui() -> ui::QrScanSchema {
        ui::QrScanSchema::User
    }

    fn try_parse(code: &str) -> Option<Self> {
        if let Ok((user_id, user_data_hash)) = decode_qr(code) {
            return Some(Self {
                user_id: user_id.hyphenated().to_string(),
                signup_extension: false,
                signup_extension_config: None,
                user_data_hash: Some(user_data_hash),
            });
        }
        if let Some(captures) = QR_CODE_V2.captures(code) {
            return Some(Data::from_v2(&captures));
        }
        #[cfg(feature = "internal-data-acquisition")]
        if let Some(captures) = QR_CODE_SIGNUP_EXTENSION.captures(code) {
            return Some(Data::from_signup_extension(&captures));
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
        Self {
            user_id,
            signup_extension: false,
            signup_extension_config: None,
            user_data_hash: None,
        }
    }

    #[cfg(feature = "internal-data-acquisition")]
    #[must_use]
    fn from_signup_extension(captures: &Captures) -> Self {
        let v2_data = Self::from_v2(captures);
        let mode = SignupMode::parse(captures.name("mode").map(|mode_group| mode_group.as_str()));
        let parameters = captures
            .name("parameters")
            .map(|parameters_group| parameters_group.as_str().to_string());

        Self {
            user_id: v2_data.user_id,
            signup_extension: true,
            signup_extension_config: mode.map(|mode| SignupExtensionConfig { mode, parameters }),
            user_data_hash: None,
        }
    }

    /// Returns true if qr code specifies signup extension mode.
    #[must_use]
    pub fn signup_extension(&self) -> bool {
        self.signup_extension
    }
}

#[cfg(feature = "internal-data-acquisition")]
impl SignupMode {
    /// Parse SignupMode enum from string
    fn parse(mode: Option<&str>) -> Option<Self> {
        if let Some(mode) = mode {
            let val = u8::from_str_radix(mode, 16).ok()?;
            let mode = match val {
                0 => Self::Basic,
                1 => Self::PupilContractionExtension,
                2 => Self::FocusSweepExtension,
                3 => Self::MirrorSweepExtension,
                4 => Self::MultiWavelength,
                5 => Self::Overcapture,
                _ => return None,
            };
            tracing::warn!("Parsed signup mode {:?} from QR code - signup flow modified!", mode);
            Some(mode)
        } else {
            None
        }
    }
}

#[cfg(feature = "internal-data-acquisition")]
impl fmt::Display for SignupMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignupMode::Basic => write!(f, "basic"),
            SignupMode::PupilContractionExtension => write!(f, "pupil_contraction"),
            SignupMode::FocusSweepExtension => write!(f, "focus_sweep"),
            SignupMode::MirrorSweepExtension => write!(f, "mirror_sweep"),
            SignupMode::MultiWavelength => write!(f, "multi_wavelength"),
            SignupMode::Overcapture => write!(f, "overcapture"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::plans::qr_scan::{operator::DUMMY_OPERATOR_QR_CODE, user::QR_CODE_SIGNUP_EXTENSION};

    use super::*;

    #[test]
    fn test_cli_operator() {
        let data = Data::from_v2(&QR_CODE_V2.captures(DUMMY_OPERATOR_QR_CODE).unwrap());
        assert_eq!(data.user_id, "66ad4897-0ca7-4727-8365-ca808348e3cd");
        assert!(data.signup_extension_config.is_none());
    }

    #[test]
    fn test_cli_user() {
        let (uuid, _) = decode_qr(DUMMY_USER_QR_CODE).unwrap();
        assert_eq!(uuid.hyphenated().to_string(), "6943c6d9-48bf-4f29-9b60-48c63222e3ea");
    }

    #[test]
    fn test_v2() {
        let text = "userid:3bcf883d-ce22-4a03-8608-4a8a01b88d4d:1";
        let data = Data::from_v2(&QR_CODE_V2.captures(text).unwrap());
        assert_eq!(data.user_id, "3bcf883d-ce22-4a03-8608-4a8a01b88d4d");
        assert!(data.signup_extension_config.is_none());
    }

    #[test]
    fn test_v2_shortened() {
        let text = "userid:3bcf883d-ce22-4a03:1";
        let data = Data::from_v2(&QR_CODE_V2.captures(text).unwrap());
        assert_eq!(data.user_id, "3bcf883d-ce22-4a03");
        assert!(data.signup_extension_config.is_none());
    }

    #[test]
    fn test_data_acquisition_malformed() {
        let text = "userid:data_acquisition_mode_focus_sweep:1:::7";
        let captures = &QR_CODE_SIGNUP_EXTENSION.captures(text);
        assert!(captures.is_none());
    }

    #[test]
    fn test_data_acquisition_malformed_2() {
        let text = "userid:data_acquisition_mode_focus_sweep::";
        let captures = &QR_CODE_SIGNUP_EXTENSION.captures(text);
        assert!(captures.is_none());
    }

    #[cfg(feature = "internal-data-acquisition")]
    mod data_acquisition_tests {
        use super::*;

        #[test]
        fn test_data_acquisition_no_optionals() {
            let text = "userid:12345678-1234-1234-1234-123456789012:1::";
            let data =
                Data::from_signup_extension(&QR_CODE_SIGNUP_EXTENSION.captures(text).unwrap());
            assert_eq!(data.user_id, "12345678-1234-1234-1234-123456789012");
            assert!(data.signup_extension_config.is_none());
        }

        #[test]
        fn test_data_acquisition_is_opt_out_by_default() {
            let text = "userid:12345678-1234-1234-1234-123456789012:0::0::";
            let data =
                Data::from_signup_extension(&QR_CODE_SIGNUP_EXTENSION.captures(text).unwrap());
            assert_eq!(data.user_id, "12345678-1234-1234-1234-123456789012");
            assert!(data.signup_extension_config.is_some());
            assert_eq!(data.signup_extension_config.as_ref().unwrap().mode, SignupMode::Basic);
            assert!(data.signup_extension_config.unwrap().parameters.is_none());
        }

        #[test]
        fn test_data_acquisition_mode_param() {
            let text = "userid:12345678-1234-1234-1234-123456789012:1::0:param::";
            let data =
                Data::from_signup_extension(&QR_CODE_SIGNUP_EXTENSION.captures(text).unwrap());
            assert_eq!(data.user_id, "12345678-1234-1234-1234-123456789012");
            assert!(data.signup_extension_config.is_some());
            assert_eq!(data.signup_extension_config.as_ref().unwrap().mode, SignupMode::Basic);
            assert_eq!(data.signup_extension_config.unwrap().parameters, Some("param".to_string()));
        }

        #[test]
        fn test_data_acquisition_non_uuid() {
            let text = "userid:data_acquisition_mode_focus_sweep:1::2:7::";
            let data =
                Data::from_signup_extension(&QR_CODE_SIGNUP_EXTENSION.captures(text).unwrap());
            assert_eq!(data.user_id, "data_acquisition_mode_focus_sweep");
            assert!(data.signup_extension_config.is_some());
            assert_eq!(
                data.signup_extension_config.as_ref().unwrap().mode,
                SignupMode::FocusSweepExtension
            );
            assert_eq!(data.signup_extension_config.unwrap().parameters, Some("7".to_string()));
        }

        #[test]
        fn test_data_acquisition_double_params() {
            let text = "userid:data_acquisition_mode_overcapture:1::5:7:20500::";
            let data =
                Data::from_signup_extension(&QR_CODE_SIGNUP_EXTENSION.captures(text).unwrap());
            assert_eq!(
                data.signup_extension_config.as_ref().unwrap().mode,
                SignupMode::Overcapture
            );
            assert_eq!(
                data.signup_extension_config.unwrap().parameters,
                Some("7:20500".to_string())
            );
        }
    }
}
