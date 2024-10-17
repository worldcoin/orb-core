//! Orb OS version validation endpoint.

use super::endpoints::MANAGEMENT_BACKEND_URL;
use crate::identification::{get_orb_token, ORB_ID, ORB_OS_VERSION};
use eyre::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrbOsVersionCheckRequest {
    orb_id: String,
    orb_os_version: String,
}

/// Version status.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrbOsVersionStatus {
    /// Version is ok.
    Allowed,
    /// Ok, but strongly recommended to update.
    Deprecated,
    /// Too old for doing signups.
    Blocked,
    /// Backend doesn't know about this version
    #[default]
    Unknown,
    /// An error occurred.
    Error,
}

/// The response from the Orb OS version validation endpoint.
#[derive(Deserialize)]
pub struct OrbOsVersionCheckResponse {
    /// The status of the Orb OS version.
    pub status: OrbOsVersionStatus,
    /// An error message.
    pub error: Option<String>,
}

/// Makes a validation request.
pub async fn request() -> Result<OrbOsVersionCheckResponse> {
    let request = OrbOsVersionCheckRequest {
        orb_id: ORB_ID.to_string(),
        orb_os_version: ORB_OS_VERSION.to_string(),
    };

    let response = super::client()?
        .post(format!("{}/api/v1/orbs/check/orbos/version", *MANAGEMENT_BACKEND_URL))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?))
        .json(&request)
        .send()
        .await?
        .error_for_status()?;

    response.json::<OrbOsVersionCheckResponse>().await.map_err(Into::into)
}
