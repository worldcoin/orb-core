//! Communication with the Orb backend.

pub mod config;
pub mod endpoints;
pub mod operator_status;
pub mod orb_os_status;
pub mod presigned_url;
pub mod s3_region;
pub mod signup_poll;
pub mod signup_post;
pub mod status;
pub mod upload_debug_report;
pub mod upload_image;
pub mod upload_personal_custody_package;
pub mod user_status;

use eyre::Error;
use std::time::Duration;
const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Creates a new HTTPS client.
pub fn client() -> reqwest::Result<reqwest::Client> {
    client_with_timeouts(REQUEST_TIMEOUT, CONNECT_TIMEOUT)
}

/// Creates a new HTTPS client with custom timeouts.
pub fn client_with_timeouts(
    request_timeout: Duration,
    connect_timeout: Duration,
) -> reqwest::Result<reqwest::Client> {
    orb_security_utils::reqwest::http_client_builder()
        .user_agent(APP_USER_AGENT)
        .timeout(request_timeout)
        .connect_timeout(connect_timeout)
        .build()
}

/// Logs an reqwest::Error error and its cause.
pub fn log_decoding_error(err: &Error) {
    let log_msg = if matches!(err.downcast_ref::<reqwest::Error>(), Some(err) if err.is_decode()) {
        "Decoding network response failed"
    } else {
        "Network request failed"
    };
    tracing::error!("{log_msg}: {err:?}", log_msg = log_msg, err = err);
}
