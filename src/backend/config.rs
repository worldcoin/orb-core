//! Orb config endpoint.

use std::collections::HashMap;

use crate::{
    agents::python::face_identifier,
    backend::endpoints::MANAGEMENT_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
    plans::fraud_check,
};
use eyre::Result;
use serde::Deserialize;

/// Response of the orb config endpoint.
#[allow(missing_docs)]
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Response {
    pub config: Config,
}

/// See [`Config`](crate::config::Config) for individual field docs.
#[allow(missing_docs, clippy::struct_excessive_bools)]
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    pub sound_volume: u64,
    pub language: Option<String>,
    pub operation_country: Option<String>,
    pub operation_city: Option<String>,
    pub fan_max_speed: Option<f32>,
    pub slow_internet_ping_threshold: Option<u64>,
    #[serde(default)]
    pub block_signup_when_no_internet: bool,
    pub ir_eye_save_fps_override: Option<f32>,
    pub ir_face_save_fps_override: Option<f32>,
    pub thermal_save_fps_override: Option<f32>,
    pub contact_lens_model_config: Option<String>,
    #[serde(flatten)]
    pub fraud_check_engine_config: fraud_check::BackendConfig,
    pub ir_net_model_configs: Option<HashMap<String, String>>,
    pub iris_model_configs: Option<HashMap<String, String>>,
    pub child_threshold: Option<f32>,
    #[serde(flatten)]
    pub face_identifier_model_configs: face_identifier::types::BackendConfig,
    /// In milliseconds
    pub thermal_camera_pairing_status_timeout: Option<u64>,
    pub thermal_camera: Option<bool>,
    pub depth_camera: Option<bool>,
    pub self_serve: Option<bool>,
    pub self_serve_button: Option<bool>,
    pub self_serve_ask_op_qr_for_possibly_underaged: Option<bool>,
    pub self_serve_ask_op_qr_for_possibly_underaged_timeout: Option<u64>,
    pub self_serve_app_skip_capture_trigger: Option<bool>,
    pub self_serve_app_capture_trigger_timeout: Option<u64>,
    pub self_serve_biometric_capture_timeout: Option<u64>,
    pub mirror_default_phi_offset_degrees: Option<f64>,
    pub mirror_default_theta_offset_degrees: Option<f64>,
    pub process_agent_logger_pruning: Option<bool>,
    pub backend_http_request_timeout: Option<u64>,
    pub backend_http_connect_timeout: Option<u64>,
    pub pcp_v3: Option<bool>,
    pub pcp_tier1_blocking_threshold: Option<u32>,
    pub pcp_tier1_dropping_threshold: Option<u32>,
    pub pcp_tier2_blocking_threshold: Option<u32>,
    pub pcp_tier2_dropping_threshold: Option<u32>,
    pub ignore_user_centric_signups: Option<bool>,
    pub user_qr_validation_use_full_operator_qr: Option<bool>,
    pub user_qr_validation_use_only_operator_location: Option<bool>,
    pub orb_relay_shutdown_wait_for_pending_messages: Option<u64>,
    pub orb_relay_shutdown_wait_for_shutdown: Option<u64>,
    pub orb_relay_announce_orb_id_retries: Option<u32>,
    pub orb_relay_announce_orb_id_timeout: Option<u64>,
    pub operator_qr_expiration_time: Option<u64>,
    pub last_updated: u64,
}

/// Makes an orb config request.
pub async fn request() -> Result<Response> {
    let request = super::client()?
        .get(format!("{}/api/v1/orbs/{}", *MANAGEMENT_BACKEND_URL, *ORB_ID))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    match request.send().await?.error_for_status() {
        Ok(response) => Ok(response.json().await?),
        Err(err) => {
            tracing::error!("Received error response {:?}", err);
            Err(err.into())
        }
    }
}
