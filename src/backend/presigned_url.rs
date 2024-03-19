//! Signup endpoint.

use crate::identification::{get_orb_token, ORB_ID};
use eyre::Result;
use orb_wld_data_id::{ImageId, SignupId};
use serde::{Deserialize, Serialize};

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

/// The presinged URL request response
#[allow(missing_docs)]
#[derive(Deserialize, Debug)]
pub struct Response {
    pub url: String,
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
    /// Self-custody images package.
    #[serde(rename = "package")]
    Package,
    /// Normalized Iris image.
    #[serde(rename = "normalizediris_image")]
    NormalizedIrisImage,
    /// Normalized Iris mask.
    #[serde(rename = "normalizediris_mask")]
    NormalizedIrisMask,
}

/// Request a presigned url
///
/// # Panics
///
/// When `url_type` is `UrlType::Package`, and `session_id` or `checksum` is
/// `None`.
pub async fn request(
    backend_url: &str,
    signup_id: &SignupId,
    image_id: Option<&ImageId>,
    session_id: Option<&str>,
    checksum: Option<&str>,
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
        UrlType::Package => {
            format!("{backend_url}/api/v2/signups/{signup_id}/package")
        }
    };
    let request = super::client()?.post(endpoint).basic_auth(&*ORB_ID, Some(get_orb_token()?));
    let request = if matches!(url_type, UrlType::Package) {
        request.json(&PackageRequest {
            orb_id: ORB_ID.as_str(),
            session_id: session_id.expect("session_id to be provided when the url type is package"),
            checksum: checksum.expect("checksum to be provided when the url type is package"),
        })
    } else {
        request.json(&Request { url_type, orb_id: ORB_ID.as_str(), image_id: &image_id })
    };
    tracing::debug!("Sending request {:#?}", request);
    match request.send().await?.error_for_status() {
        Ok(res) => {
            let res = res.json::<Response>().await?;
            tracing::debug!("Received response {:#?}", res);
            Ok(res)
        }
        Err(e) => {
            tracing::error!("Received error response {:#?}", e);
            Err(e.into())
        }
    }
}
