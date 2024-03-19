//! Signup create endpoint.

use super::user_status;
use crate::{
    agents::{
        camera::{Frame, FrameResolution},
        image_notary::IdentificationImages,
    },
    backend::endpoints::SIGNUP_BACKEND_URL,
    identification::{get_orb_token, ORB_ID, OVERALL_SOFTWARE_VERSION},
    logger::{DATADOG, NO_TAGS},
    plans::{
        biometric_capture::{Capture, EyeCapture},
        biometric_pipeline::{EyePipeline, Pipeline},
        qr_scan,
    },
};
use eyre::Result;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::{
    convert::TryInto,
    io::Cursor,
    time::{Duration, SystemTime},
};
use tokio::task;

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
    /// Signup failed due to some agent dying or some internal error.
    Failure,
    /// Signup was detected as a fraud attempt.
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
    user_data: &user_status::UserData,
    s3_region: &str,
    capture: &Capture,
    pipeline: Option<&Pipeline>,
    identification_image_ids: Option<&IdentificationImages>,
    signup_reason: SignupReason,
) -> Result<Response> {
    DATADOG.gauge(
        "orb.main.gauge.signup.sharpest_iris",
        capture.eye_left.ir_net_estimate.score.to_string(),
        ["side:left"],
    )?;
    DATADOG.gauge(
        "orb.main.gauge.signup.sharpest_iris",
        capture.eye_right.ir_net_estimate.score.to_string(),
        ["side:right"],
    )?;
    let opt_in = user_data.data_policy.is_opt_in() && identification_image_ids.is_some();
    if user_data.data_policy.is_opt_in() {
        DATADOG.incr("orb.main.count.data_collection.opt_in", NO_TAGS)?;
    } else {
        DATADOG.incr("orb.main.count.data_collection.opt_out", NO_TAGS)?;
    }
    tracing::info!("Overall software version: {:?}", &*OVERALL_SOFTWARE_VERSION);
    tracing::info!("Signup reason: {:?}", signup_reason);
    let codes = pipeline.map_or(String::new(), |p| {
        serde_json::to_string_pretty(&format_pipeline(p)).expect("always a valid JSON")
    });
    let mut form = Form::new()
        .text("softwareVersion", &*OVERALL_SOFTWARE_VERSION)
        .text("orbId", ORB_ID.as_str())
        .text("distributorId", operator_qr_code.user_id.clone())
        .text("userId", user_qr_code.user_id.clone())
        .text("region", s3_region.to_owned())
        .text("optIn", if opt_in { "true" } else { "false" })
        .text("signature", signature.map_or(String::default(), Clone::clone))
        .text("codes", codes)
        .text("reason", signup_reason.to_screaming_snake_case().to_string());
    if opt_in {
        let (iris_left, iris_right, faces) =
            encode_images(capture.eye_left.clone(), capture.eye_right.clone()).await?;
        form = form.part("irisLeftImages", iris_left).part("irisRightImages", iris_right);
        for face in faces {
            form = form.part("faceImages", face);
        }
        if let Some(identification_image_ids) = identification_image_ids {
            form = form
                .text("irLeftImageId", identification_image_ids.left_ir.to_string())
                .text("irRightImageId", identification_image_ids.right_ir.to_string())
                .text("faceOneImageId", identification_image_ids.left_rgb.to_string())
                .text("faceTwoImageId", identification_image_ids.right_rgb.to_string());
        }
    }
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
    DATADOG.timing(
        "orb.main.time.http.signup_request",
        t.elapsed().unwrap_or(Duration::MAX).as_millis().try_into()?,
        NO_TAGS,
    )?;
    if let Some(request_size) = request_size {
        DATADOG.gauge(
            "orb.main.time.http.signup_request_size",
            request_size.to_str().unwrap_or("0"),
            NO_TAGS,
        )?;
    }
    tracing::debug!("Received response {:#?}", response);
    Ok(response)
}

async fn encode_images(
    eye_left: EyeCapture,
    eye_right: EyeCapture,
) -> Result<(Part, Part, Vec<Part>)> {
    task::spawn_blocking(move || -> Result<_> {
        let mut left_ir_png = Cursor::new(Vec::new());
        let mut left_rgb_png = Cursor::new(Vec::new());
        let mut right_ir_png = Cursor::new(Vec::new());
        let mut right_rgb_png = Cursor::new(Vec::new());
        eye_left.ir_frame.write_png(&mut left_ir_png, FrameResolution::MAX)?;
        eye_left.rgb_frame.write_png(&mut left_rgb_png, FrameResolution::MEDIUM)?;
        eye_right.ir_frame.write_png(&mut right_ir_png, FrameResolution::MAX)?;
        eye_right.rgb_frame.write_png(&mut right_rgb_png, FrameResolution::LOW)?;
        Ok((
            Part::bytes(left_ir_png.into_inner())
                .file_name(format!("l_{}.png", eye_left.ir_net_estimate.score)),
            Part::bytes(right_ir_png.into_inner())
                .file_name(format!("r_{}.png", eye_right.ir_net_estimate.score)),
            vec![
                Part::bytes(left_rgb_png.into_inner()).file_name("f_0.png"),
                Part::bytes(right_rgb_png.into_inner()).file_name("f_1.png"),
            ],
        ))
    })
    .await?
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
