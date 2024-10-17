//! Signup create endpoint.

use crate::{
    backend::endpoints::SIGNUP_BACKEND_URL,
    dd_gauge, dd_timing,
    identification::{get_orb_token, ORB_ID, ORB_OS_VERSION},
    plans::{
        biometric_capture::Capture,
        biometric_pipeline::{EyePipeline, Pipeline},
        qr_scan,
    },
};
use eyre::Result;
use reqwest::multipart::Form;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// The "codes" request form field from the V2 pipeline.
#[derive(Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct CodesV2 {
    /// IR-Net version.
    pub ir_net: String,
    /// Iris version.
    pub iris: String,
    /// Codes and masks for the left eye (1 element vector).
    /// This is a Vec because historically orb-core was calculating all iris rotations instead of the backend.
    pub left: Vec<IrisData>,
    /// Codes and masks for the right eye (1 element vector).
    /// This is a Vec because historically orb-core was calculating all iris rotations instead of the backend.
    pub right: Vec<IrisData>,
}

/// Iris data.
#[derive(Serialize, Debug)]
pub struct IrisData {
    /// Iris code.
    pub code: String,
    /// Iris mask.
    pub mask: String,
    /// Iris code version.
    pub code_version: String,
}

/// Response of the signup endpoint.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Status of the Orb versions.
    #[serde(default)]
    pub software_version_status: SoftwareVersionStatus,
}

/// Versions status.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SoftwareVersionStatus {
    /// Versions are ok.
    #[default]
    Allowed,
    /// Ok, but strongly recommended to update.
    Deprecated,
    /// Too old for doing signups.
    Blocked,
    /// Backend doesn't know about this version
    Unknown,
    /// Backend sometimes returns empty string
    #[serde(alias = "")]
    Empty,
}

/// Every signup needs to be tagged with a reason for the backend to process it.
#[derive(Serialize, Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum SignupReason {
    /// Signup was successfully processed on the Orb.
    #[default]
    Normal,
    /// Signup failed due to some agent dying in the biometric pipeline or some internal error.
    Failure,
    /// Signup was detected as a fraud attempt at the orb (not to be confused with the backend fraud checks).
    Fraud,
}

/// Converts the signup reason to screaming snake case.
impl SignupReason {
    /// Converts the signup reason to screaming snake case. Using Serde's renaming won't work because Serde
    /// automatically adds "" (quotes) to the produced string. I.e. the output from Serde is "\"NORMAL\"".
    #[must_use]
    pub fn to_screaming_snake_case(&self) -> &str {
        match self {
            SignupReason::Normal => "NORMAL",
            SignupReason::Failure => "FAILURE",
            SignupReason::Fraud => "FRAUD",
        }
    }
}

/// Makes a signup request.
#[allow(clippy::too_many_arguments)]
pub async fn request(
    signature: Option<&String>,
    signup_id: &str,
    operator_qr_code: &qr_scan::user::Data,
    user_qr_code: &qr_scan::user::Data,
    s3_region: &str,
    capture: &Capture,
    pipeline: Option<&Pipeline>,
    signup_reason: SignupReason,
) -> Result<Response> {
    dd_gauge!(
        "main.gauge.signup.sharpest_iris",
        capture.eye_left.ir_net_estimate.score.to_string(),
        "side:left"
    );
    dd_gauge!(
        "main.gauge.signup.sharpest_iris",
        capture.eye_right.ir_net_estimate.score.to_string(),
        "side:right"
    );
    tracing::info!("Orb OS version: {:?}", &*ORB_OS_VERSION);
    tracing::info!("Signup reason: {:?}", signup_reason);
    let codes = pipeline.map_or(String::new(), |p| {
        serde_json::to_string_pretty(&format_pipeline(p)).expect("always a valid JSON")
    });
    let mut form = Form::new()
        .text("softwareVersion", &*ORB_OS_VERSION)
        .text("orbId", ORB_ID.as_str())
        .text("distributorId", operator_qr_code.user_id.clone())
        .text("userId", user_qr_code.user_id.clone())
        .text("region", s3_region.to_owned())
        .text("signature", signature.map_or(String::default(), Clone::clone))
        .text("codes", codes)
        .text("reason", signup_reason.to_screaming_snake_case().to_string());
    if let Some(latitude) = capture.latitude {
        form = form.text("latitude", latitude.to_string());
    }
    if let Some(longitude) = capture.longitude {
        form = form.text("longitude", longitude.to_string());
    }
    let request = super::client()?
        .post(format!("{}/api/v2/signups/{signup_id}", *SIGNUP_BACKEND_URL))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?))
        .multipart(form);

    let request = request.build()?;
    let headers = request.headers().clone();
    let request_size = headers.get("Content-Length");
    tracing::debug!("Sending request {:#?} with size: {:?}", request, request_size);

    let t = SystemTime::now();
    let response = super::client()?.execute(request).await?;
    tracing::debug!("Received response {:#?}", response);
    response.error_for_status_ref()?;
    let response = response.json::<Response>().await?;
    dd_timing!("main.time.http.signup_request", t);
    if let Some(request_size) = request_size {
        dd_gauge!("main.time.http.signup_request_size", request_size.to_str().unwrap_or("0"));
    }
    tracing::debug!("Received response {:#?}", response);
    Ok(response)
}

/// Serializes pipeline outputs into backend format.
#[must_use]
pub fn format_pipeline(pipeline: &Pipeline) -> Vec<CodesV2> {
    vec![CodesV2 {
        left: format_eye_pipeline(&pipeline.v2.eye_left),
        right: format_eye_pipeline(&pipeline.v2.eye_right),
        ir_net: pipeline.v2.ir_net_version.clone(),
        iris: pipeline.v2.iris_version.clone(),
    }]
}

fn format_eye_pipeline(eye: &EyePipeline) -> Vec<IrisData> {
    vec![IrisData {
        code: eye.iris_code.clone(),
        mask: eye.mask_code.clone(),
        code_version: eye.iris_code_version.clone(),
    }]
}
