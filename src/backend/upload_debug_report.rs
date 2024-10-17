//! Upload signup data via presigned URL.

use crate::{
    backend::{
        endpoints::DATA_BACKEND_URL,
        presigned_url::{self, UrlType},
    },
    dd_timing,
    debug_report::DebugReport,
};
use eyre::Result;
use flate2::{write::GzEncoder, Compression};
use orb_wld_data_id::SignupId;
use std::time::SystemTime;

/// Compresses and uploads the signup JSON.
pub async fn request(signup_id: &SignupId, debug_report: &DebugReport) -> Result<()> {
    let t0 = SystemTime::now();
    let presigned_url::Response { url: presigned_url, .. } =
        presigned_url::request(&DATA_BACKEND_URL, signup_id, None, UrlType::Metadata).await?;
    dd_timing!("main.time.data_acquisition.upload.signup_json.presigned", t0);
    tracing::debug!("Metadata presigned_url: {:?}", presigned_url);
    let request = super::client()?
        .put(presigned_url)
        .header("content-type", "application/json")
        .header("content-encoding", "gzip")
        .body(compressed_signup_json(debug_report)?);
    tracing::debug!("Sending request {:#?}", request);
    let t1 = SystemTime::now();
    let response = request.send().await?;
    dd_timing!("main.time.data_acquisition.upload.signup_json.upload", t1);
    tracing::debug!("Received response {:#?}", response);
    response.error_for_status()?;
    Ok(())
}

fn compressed_signup_json(debug_report: &DebugReport) -> Result<Vec<u8>> {
    let mut compressed_debug_report = Vec::new();
    let mut encoder = GzEncoder::new(&mut compressed_debug_report, Compression::default());
    serde_json::to_writer(&mut encoder, &debug_report)?;
    encoder.finish()?;
    Ok(compressed_debug_report)
}
