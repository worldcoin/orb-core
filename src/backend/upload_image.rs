//! Upload image via presigned URL.

use crate::{
    backend::{endpoints::DATA_BACKEND_URL, presigned_url, presigned_url::UrlType},
    logger::{inst_elapsed, LogOnError, DATADOG, NO_TAGS},
};
use eyre::Result;
use orb_wld_data_id::{ImageId, SignupId};
use reqwest::header::CONTENT_LENGTH;
use std::{convert::TryInto, time::Instant};

/// Uploads an image.
pub async fn request(
    signup_id: &SignupId,
    image_id: &ImageId,
    presigned_url_type: UrlType,
    img_data: Vec<u8>,
    dd_image_type: &str,
) -> Result<()> {
    let t: Instant = Instant::now();
    let presigned_url::Response { url: presigned_url } = presigned_url::request(
        &DATA_BACKEND_URL,
        signup_id,
        Some(image_id),
        None,
        None,
        presigned_url_type,
    )
    .await?;
    DATADOG
        .timing(
            format!("orb.main.time.data_collection.upload.{dd_image_type}.presigned"),
            inst_elapsed!(t),
            NO_TAGS,
        )
        .or_log();
    tracing::debug!("Image presigned_url: {:?}", presigned_url);
    let request =
        super::client()?.put(presigned_url).header(CONTENT_LENGTH, img_data.len()).body(img_data);
    let t = Instant::now();
    let response = request.send().await?;
    DATADOG
        .timing(
            format!("orb.main.time.data_collection.upload.{dd_image_type}.upload"),
            inst_elapsed!(t),
            NO_TAGS,
        )
        .or_log();
    response.error_for_status()?;
    Ok(())
}
