//! Operator ID validation endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
    plans::qr_scan,
};
use eyre::Result;
use serde::Deserialize;

/// Coordinates.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Coordinates {
    /// Latitude.
    pub latitude: f64,
    /// Longitude.
    pub longitude: f64,
}

/// Location data of the operator.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocationData {
    /// The operator's team country.
    pub team_operating_country: String,
    /// The operator's coordinates during the session.
    pub session_coordinates: Coordinates,
    /// The operator's expected stationary location coordinates.
    pub stationary_location_coordinates: Option<Coordinates>,
}

/// Operator ID validation status.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether the operator ID is valid.
    pub valid: bool,
    /// Location data of the operator.
    pub location_data: Option<LocationData>,
    /// If 'valid == false', the 'reason' field contains the reason for the invalidation.
    pub reason: Option<String>,
}

/// Makes a validation request.
pub async fn request(qr_code: &qr_scan::user::Data) -> Result<Status> {
    let request = super::client()?
        .get(format!(
            "{}/api/v1/distributor/{}/orb/{}/status",
            *SIGNUP_BACKEND_URL, qr_code.user_id, *ORB_ID
        ))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let status: Status = match request.send().await?.error_for_status() {
        Ok(response) => response.json().await?,
        Err(err) => {
            tracing::error!("Received error response {err:?}");
            return Err(err.into());
        }
    };
    if !status.valid {
        tracing::info!(
            "Operator QR-code invalid: {qr_code:?}, reason: {:?}",
            status.reason.as_deref().unwrap_or("<empty>")
        );
        return Ok(status);
    }
    if status.location_data.is_none() {
        tracing::error!("Operator location data are missing");
        return Ok(Status {
            valid: false,
            location_data: None,
            reason: Some("Operator location data are missing".to_string()),
        });
    }
    Ok(status)
}
