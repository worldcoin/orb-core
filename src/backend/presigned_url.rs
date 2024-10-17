//! Signup endpoint.

use crate::identification::{get_orb_token, ORB_ID};
use eyre::Result;
use orb_wld_data_id::{ImageId, SignupId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The JSON structure of the presigned URL request.
#[allow(missing_docs)]
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request<'a> {
    // Struct fields named "type" collide with the Rust keyword
    #[serde(rename = "type")]
    pub url_type: UrlType,
    pub orb_id: &'a str,
    pub image_id: &'a str,
}

/// The JSON structure of the self-custody package presigned URL request.
#[allow(missing_docs)]
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PackageRequest<'a> {
    pub orb_id: &'a str,
    pub session_id: &'a str,
    pub checksum: &'a str,
}

/// The JSON structure of the tiered self-custody package presigned URL request.
#[allow(missing_docs)]
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TieredPackageRequest<'a> {
    pub orb_id: &'a str,
    pub session_id: &'a str,
    pub checksum: &'a str,
    pub tier: u8,
}

/// The presinged URL request response
#[allow(missing_docs)]
#[derive(Deserialize, Debug)]
pub struct Response {
    pub url: String,
    pub fields: Option<HashMap<String, String>>,
}

/// The type of presigned URL to request
/// This determines which S3 bucket to use for the uploaded data
/// See the presignedRequest function in upload_endpoints.go in orb-service
#[derive(Clone, Copy, Debug, Serialize)]
pub enum UrlType {
    /// metadata (i.e. signup JSON)
    #[serde(rename = "metadata")]
    Metadata,
    /// IR images
    #[serde(rename = "ir")]
    Ir,
    /// RGB images
    #[serde(rename = "rgb")]
    Rgb,
    /// Thermal camera images
    #[serde(rename = "heat")]
    Thermal,
    /// Face IR camera images
    #[serde(rename = "front-ir")]
    IrFace,
    /// 2D TOF ir / greyscale camera images
    #[serde(rename = "tof2d_ir")]
    Tof2dIr,
    /// 2D TOF confidence values
    #[serde(rename = "tof2d_confidence")]
    Tof2dConfidence,
    /// 2D TOF depth map (z values)
    #[serde(rename = "tof2d_depth")]
    Tof2dDepth,
    /// 2D TOF noise metric
    #[serde(rename = "tof2d_noise")]
    Tof2dNoise,
    /// Normalized Iris image.
    #[serde(rename = "normalizediris_image")]
    NormalizedIrisImage,
    /// Normalized Iris mask.
    #[serde(rename = "normalizediris_mask")]
    NormalizedIrisMask,
}

/// Request a presigned url
pub async fn request(
    backend_url: &str,
    signup_id: &SignupId,
    image_id: Option<&ImageId>,
    url_type: UrlType,
) -> Result<Response> {
    let image_id = image_id.map(ToString::to_string).unwrap_or_default();
    let endpoint = match url_type {
        UrlType::Ir
        | UrlType::Rgb
        | UrlType::Thermal
        | UrlType::IrFace
        | UrlType::Tof2dIr
        | UrlType::Tof2dDepth
        | UrlType::NormalizedIrisImage
        | UrlType::NormalizedIrisMask => {
            format!("{backend_url}/api/v2/signups/{signup_id}/upload")
        }
        UrlType::Metadata | UrlType::Tof2dConfidence | UrlType::Tof2dNoise => {
            format!("{backend_url}/api/v1/signups/{signup_id}/upload")
        }
    };
    let request = super::client()?.post(endpoint).basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let request = request.json(&Request { url_type, orb_id: ORB_ID.as_str(), image_id: &image_id });
    tracing::debug!("Sending request {request:#?}");
    let response = request.send().await?;
    match response.error_for_status_ref() {
        Ok(_) => {
            let response = response.json::<Response>().await?;
            tracing::debug!("Received response {response:#?}");
            Ok(response)
        }
        Err(err) => {
            let response = response.text().await?;
            tracing::error!("Received error response {err:#?} with body: {response}");
            Err(err.into())
        }
    }
}

/// Request a presigned url for "package".
pub async fn request_package(
    backend_url: &str,
    signup_id: &SignupId,
    session_id: &str,
    checksum: &str,
) -> Result<Response> {
    let endpoint = format!("{backend_url}/api/v3/signups/{signup_id}/package");
    let request = super::client()?.post(endpoint).basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let request = request.json(&PackageRequest { orb_id: ORB_ID.as_str(), session_id, checksum });
    tracing::debug!("Sending request {request:#?}");
    let response = request.send().await?;
    match response.error_for_status_ref() {
        Ok(_) => {
            let response = response.json::<Response>().await?;
            tracing::debug!("Received response {response:#?}");
            Ok(response)
        }
        Err(err) => {
            let response = response.text().await?;
            tracing::error!("Received error response {err:#?} with body: {response}");
            Err(err.into())
        }
    }
}

/// Request a presigned url for "tiered_package".
pub async fn request_tiered_package(
    backend_url: &str,
    signup_id: &SignupId,
    session_id: &str,
    checksum: &str,
    tier: u8,
) -> Result<Response> {
    let endpoint = format!("{backend_url}/api/v3/signups/{signup_id}/tiered/package");
    let request = super::client()?.post(endpoint).basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let request =
        request.json(&TieredPackageRequest { orb_id: ORB_ID.as_str(), session_id, checksum, tier });
    tracing::debug!("Sending request {request:#?}");
    let response = request.send().await?;
    match response.error_for_status_ref() {
        Ok(_) => {
            let response = response.json::<Response>().await?;
            tracing::debug!("Received response {response:#?}");
            Ok(response)
        }
        Err(err) => {
            let response = response.text().await?;
            tracing::error!("Received error response {err:#?} with body: {response}");
            Err(err.into())
        }
    }
}
