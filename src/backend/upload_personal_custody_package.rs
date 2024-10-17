//! Upload personal custody packages via presigned URL.

use crate::{
    backend::{endpoints::DATA_BACKEND_URL, presigned_url},
    config::Config,
    dd_timing,
};
use data_encoding::BASE64;
use eyre::Result;
use orb_wld_data_id::SignupId;
use reqwest::multipart::{Form, Part};
use std::{sync::Arc, time::Instant};
use tokio::sync::Mutex;

/// Uploads a personal custody package.
pub async fn request(
    signup_id: &SignupId,
    session_id: &str,
    checksum: &[u8],
    data: &[u8],
    tier: Option<u8>,
    config: &Arc<Mutex<Config>>,
) -> Result<()> {
    let t0 = Instant::now();
    let presigned_url::Response { url: presigned_url, fields: form_data_params } =
        if let Some(tier) = tier {
            presigned_url::request_tiered_package(
                &DATA_BACKEND_URL,
                signup_id,
                session_id,
                &BASE64.encode(checksum),
                tier,
            )
            .await?
        } else {
            presigned_url::request_package(
                &DATA_BACKEND_URL,
                signup_id,
                session_id,
                &BASE64.encode(checksum),
            )
            .await?
        };
    dd_timing!("main.time.signup.upload_custody_images.presigned", t0);
    tracing::debug!("Images self-custody presigned_url: {presigned_url:?}");
    tracing::debug!("Images self-custody form_data_params: {form_data_params:?}");
    let file = Part::bytes(data.to_vec())
        .file_name("package.tar.gz")
        .mime_str("application/octet-stream")?;
    let form = form_data_params
        .into_iter()
        .flatten()
        .fold(Form::new(), |form, (key, value)| form.text(key, value))
        .part("file", file);
    let Config { backend_http_connect_timeout, backend_http_request_timeout, .. } =
        *config.lock().await;
    let request =
        super::client_with_timeouts(backend_http_connect_timeout, backend_http_request_timeout)?
            .post(presigned_url)
            .multipart(form);
    tracing::debug!("Sending request {request:#?}");
    let t1 = Instant::now();
    let response = request.send().await?;
    dd_timing!("main.time.signup.upload_custody_images.upload", t1);
    tracing::debug!("Received response {response:#?}");
    response.error_for_status()?;
    Ok(())
}
