//! Operator ID validation endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
    plans::qr_scan,
};
use eyre::Result;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct Response {
    valid: bool,
    reason: Option<String>,
}

/// Makes a validation request.
pub async fn request(qr_code: &qr_scan::user::Data) -> Result<bool> {
    let request = super::client()?
        .get(format!(
            "{}/api/v1/distributor/{}/orb/{}/status",
            *SIGNUP_BACKEND_URL, qr_code.user_id, *ORB_ID
        ))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let Response { valid, reason } = match request.send().await?.error_for_status() {
        Ok(response) => response.json().await?,
        Err(err) => {
            tracing::error!("Received error response {err:?}");
            return Err(err.into());
        }
    };
    if !valid {
        tracing::info!(
            "Operator QR-code invalid: {qr_code:?}, reason: {:?}",
            reason.as_deref().unwrap_or("<empty>")
        );
        return Ok(false);
    }
    Ok(true)
}
