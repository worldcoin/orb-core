//! Orb status endpoint.

use crate::{
    backend::endpoints::MANAGEMENT_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
};
use eyre::Result;
use serde::Serialize;

/// The JSON structure of the orb status request.
#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub battery: Battery,
    pub wifi: Wifi,
    pub temperature: Temperature,
    pub location: Location,
    pub ssd: Ssd,
    pub version: OrbVersion,
    pub mac_address: String,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub level: f64,
    pub is_charging: bool,
}

impl Default for Battery {
    fn default() -> Self {
        // is_charging set to true prevents the charging sound to play on boot if the orb is plugged in
        Self { level: f64::default(), is_charging: true }
    }
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Wifi {
    #[serde(rename = "SSID")]
    pub ssid: String,
    pub quality: WifiQuality,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WifiQuality {
    pub bit_rate: f64,
    pub link_quality: i64,
    pub signal_level: i64,
    pub noise_level: i64,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Temperature {
    pub cpu: f64,
    pub gpu: f64,
    pub front_unit: f64,
    #[serde(rename = "frontPCB")]
    pub front_pcb: f64,
    pub backup_battery: f64,
    #[serde(rename = "batteryPCB")]
    pub battery_pcb: f64,
    pub battery_cell: f64,
    pub liquid_lens: f64,
    pub main_accelerometer: f64,
    #[serde(rename = "mainMCU")]
    pub main_mcu: f64,
    pub mainboard: f64,
    pub security_accelerometer: f64,
    #[serde(rename = "securityMCU")]
    pub security_mcu: f64,
    pub battery_pack: f64,
    #[serde(rename = "ssd")]
    pub ssd: f64,
    pub wifi: f64,
    pub main_board_usb_hub_bot: f64,
    pub main_board_usb_hub_top: f64,
    pub main_board_security_supply: f64,
    pub main_board_audio_amplifier: f64,
    pub power_board_super_cap_charger: f64,
    pub power_board_pvcc_supply: f64,
    pub power_board_super_caps_left: f64,
    pub power_board_super_caps_right: f64,
    pub front_unit_850_730_left_top: f64,
    pub front_unit_850_730_left_bottom: f64,
    pub front_unit_850_730_right_top: f64,
    pub front_unit_850_730_right_bottom: f64,
    pub front_unit_940_left_top: f64,
    pub front_unit_940_left_bottom: f64,
    pub front_unit_940_right_top: f64,
    pub front_unit_940_right_bottom: f64,
    pub front_unit_940_center_top: f64,
    pub front_unit_940_center_bottom: f64,
    pub front_unit_white_top: f64,
    pub front_unit_shroud_rgb_top: f64,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Ssd {
    pub file_left: i64,
    pub space_left: i64,
    pub signup_left_to_upload: i64,
}

#[allow(missing_docs)]
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrbVersion {
    pub current_release: String,
}

/// Makes an orb status request.
pub async fn request(request: &Request) -> Result<()> {
    let response = super::client()?
        .post(format!("{}/api/v1/orbs/{}/status", *MANAGEMENT_BACKEND_URL, *ORB_ID))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?))
        .json(request)
        .send()
        .await?;
    response.error_for_status_ref()?;
    Ok(())
}
