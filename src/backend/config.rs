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
    pub fan_max_speed: Option<f32>,
    pub slow_internet_ping_threshold: Option<u64>,
    #[serde(default)]
    pub block_signup_when_no_internet: bool,
    pub ir_eye_save_fps_override: Option<f32>,
    pub ir_face_save_fps_override: Option<f32>,
    pub thermal_save_fps_override: Option<f32>,
    #[serde(flatten)]
    pub fraud_check_engine_config: fraud_check::BackendConfig,
    pub ir_net_model_configs: Option<HashMap<String, String>>,
    pub iris_model_configs: Option<HashMap<String, String>>,
    #[serde(flatten)]
    pub face_identifier_model_configs: face_identifier::types::BackendConfig,
    pub thermal_camera_pairing_status_timeout: Option<u64>,
    pub thermal_camera: Option<bool>,
    pub upload_self_custody_images: Option<bool>,
    pub upload_self_custody_thumbnail: Option<bool>,
    pub upload_iris_normalized_images: Option<bool>,
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
