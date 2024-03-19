//! Signup status endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
};
use eyre::{Context, Result};
use serde::{Deserialize, Deserializer};

/// Response of the signup endpoint.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Signup status.
    pub status: Status,
    /// Successful signup.
    #[serde(default = "bool::default")]
    pub success: bool,
    /// Signup error.
    #[serde(default, deserialize_with = "empty_string_is_none")]
    pub error: Option<String>,
}

/// Signup status.
#[derive(Clone, Copy, Deserialize, Debug)]
pub enum Status {
    /// Signup created.
    Accepted,
    /// Signup in progress.
    InProgress,
    /// Retriable error.
    Error,
    /// Signup completed.
    Completed,
    /// Non-retriable error.
    Failed,
}

/// Makes a signup request.
pub async fn request(signup_id: &str) -> Result<Response> {
    let request = super::client()?
        .get(format!("{}/api/v1/signups/{signup_id}", *SIGNUP_BACKEND_URL))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    tracing::debug!("Sending request {:#?}", request);
    let response = request.send().await?;
    tracing::debug!("Received response {:#?}", response);
    response.error_for_status_ref()?;

    let response_body = response.text().await?;
    let parsed_response = serde_json::from_str::<Response>(&response_body)
        .map_err(|e| {
            tracing::error!("Failed to parse response JSON. Response body: {:#?}", response_body);
            e
        })
        .wrap_err("Failed to parse response JSON")?;

    tracing::debug!("Received response {:#?}", parsed_response);
    Ok(parsed_response)
}

fn empty_string_is_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    if string.is_empty() { Ok(None) } else { Ok(Some(string)) }
}
