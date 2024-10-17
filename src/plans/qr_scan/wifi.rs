//! WiFi QR-code scanning.
//!
//! Handle MECARD format for reading WiFi credentials from QR Codes.
//!
//! Spec:
//! <https://github.com/zxing/zxing/wiki/Barcode-Contents#wi-fi-network-config-android-ios-11>

use super::Schema;
use crate::{network::mecard::Credentials, ui};
use nom::Finish;

impl Schema for Credentials {
    fn ui() -> ui::QrScanSchema {
        ui::QrScanSchema::Wifi
    }

    fn try_parse(code: &str) -> Option<Self> {
        match Self::parse(code).finish() {
            Ok((_, credentials)) => Some(credentials),
            Err(err) => {
                tracing::debug!("WiFi credentials parse error: {:?}", err);
                None
            }
        }
    }
}
