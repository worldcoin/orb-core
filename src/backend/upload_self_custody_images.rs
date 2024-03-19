//! Upload self-custody images via presigned URL.

use crate::{
    backend::{endpoints::DATA_BACKEND_URL, presigned_url, presigned_url::UrlType},
    logger::{inst_elapsed, LogOnError, DATADOG, NO_TAGS},
};
use eyre::Result;
use orb_wld_data_id::SignupId;
use std::{convert::TryInto, time::Instant};

/// Uploads self-custody images package.
pub async fn request(
    signup_id: &SignupId,
    session_id: &str,
    checksum: &str,
    data: Vec<u8>,
) -> Result<()> {
    let t0 = Instant::now();
    let presigned_url::Response { url: presigned_url } = presigned_url::request(
        &DATA_BACKEND_URL,
        signup_id,
        None,
        Some(session_id),
        Some(checksum),
        UrlType::Package,
    )
    .await?;
    DATADOG
        .timing(
            "orb.main.time.signup.upload_self_custody_images.presigned",
            inst_elapsed!(t0),
            NO_TAGS,
        )
        .or_log();
    tracing::debug!("Images self-custody presigned_url: {presigned_url:?}");
    let request =
        super::client()?.put(presigned_url).header("x-amz-meta-checksum", checksum).body(data);
    tracing::debug!("Sending request {request:#?}");
    let t1 = Instant::now();
    let response = request.send().await?;
    DATADOG
        .timing(
            "orb.main.time.signup.upload_self_custody_images.upload",
            inst_elapsed!(t1),
            NO_TAGS,
        )
        .or_log();
    tracing::debug!("Received response {response:#?}");
    response.error_for_status()?;
    Ok(())
}
