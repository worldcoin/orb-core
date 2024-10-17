//! User enrollment.

use super::{
    biometric_capture::Capture, biometric_pipeline::Pipeline, notify_failed_signup, qr_scan,
};
use crate::{
    backend::{
        log_decoding_error, signup_poll,
        signup_post::{self, SignupReason},
    },
    brokers::Orb,
    dd_incr,
    identification::ORB_ID,
    secure_element,
    ui::SignupFailReason,
};
use data_encoding::BASE64;
use eyre::Result;
use orb_wld_data_id::SignupId;
use ring::digest::{Context, SHA256};
use schemars::JsonSchema;
use serde::Serialize;
use std::time::Duration;
use tokio::{task, time::sleep};

const RETRIES_COUNT: usize = 3;
const POLL_STATUS_COUNT: usize = 30;
const POLL_STATUS_INTERVAL: Duration = Duration::from_secs(2);

/// Status of the user enrollment.
#[derive(PartialEq, Serialize, JsonSchema, Clone)]
pub enum Status {
    /// User enrollment was successful.
    Success,
    /// User enrollment failed due to a signature calculation error.
    SignatureCalculationError,
    /// User enrollment failed due to a software version being unknown.
    SoftwareVersionUnknown,
    /// User enrollment failed due to a software version being outdated.
    SoftwareVersionOutdated,
    /// User enrollment failed due to a signup verification not being successful.
    SignupVerificationNotSuccessful,
    /// User enrollment failed due to a server error.
    ServerError,
    /// User enrollment failed due to a network or other internal error.
    Error,
}

impl Status {
    /// Returns `true` if the status is `Success`.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Status::Success)
    }
}

/// User enrollment plan.
#[allow(missing_docs)]
pub struct Plan<'a> {
    pub signup_id: SignupId,
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub s3_region_str: String,
    pub capture: &'a Capture,
    pub pipeline: Option<&'a Pipeline>,
    pub signup_reason: SignupReason,
}

impl Plan<'_> {
    /// Runs the user enrollment plan.
    #[allow(clippy::too_many_lines)]
    pub async fn run(self, orb: &mut Orb) -> Status {
        let user_qr_code = self.user_qr_code.clone();
        let signature = if let Some(p) = self.pipeline.cloned() {
            match task::spawn_blocking(move || make_signature(&user_qr_code, &p)).await {
                Ok(Ok(signature)) => Some(signature),
                Ok(Err(err)) => {
                    tracing::error!("Failed to calculate signature: {err:?}");
                    return Status::SignatureCalculationError;
                }
                Err(err) => {
                    tracing::error!("Failed to calculate signature: {err:?}");
                    return Status::SignatureCalculationError;
                }
            }
        } else {
            None
        };
        tracing::info!("Iris code signature: {:?}", signature);
        let signup_id = self.signup_id.to_string();
        for i in 0..RETRIES_COUNT {
            let response = signup_post::request(
                signature.as_ref(),
                &signup_id,
                &self.operator_qr_code,
                &self.user_qr_code,
                &self.s3_region_str,
                self.capture,
                self.pipeline,
                self.signup_reason,
            )
            .await;
            match response {
                Ok(signup_post::Response {
                    software_version_status:
                        versions @ (signup_post::SoftwareVersionStatus::Allowed
                        | signup_post::SoftwareVersionStatus::Deprecated
                        | signup_post::SoftwareVersionStatus::Unknown
                        | signup_post::SoftwareVersionStatus::Empty),
                }) => {
                    if matches!(versions, signup_post::SoftwareVersionStatus::Deprecated) {
                        tracing::warn!("Orb component versions are deprecated");
                        notify_failed_signup(
                            orb,
                            Some(SignupFailReason::SoftwareVersionDeprecated),
                        );
                    }
                    if matches!(versions, signup_post::SoftwareVersionStatus::Empty)
                        || matches!(versions, signup_post::SoftwareVersionStatus::Unknown)
                    {
                        tracing::warn!("Backend doesn't know this software version.");
                        tracing::warn!(
                            "This is considered a deprecated version on staging builds, and \
                             blocked on prod."
                        );
                        #[cfg(feature = "stage")]
                        notify_failed_signup(
                            orb,
                            Some(SignupFailReason::SoftwareVersionDeprecated),
                        );
                        #[cfg(not(feature = "stage"))]
                        return Status::SoftwareVersionUnknown;
                    }
                    for i in 0..POLL_STATUS_COUNT {
                        sleep(POLL_STATUS_INTERVAL).await;
                        #[cfg(not(feature = "ui-test-successful-signup"))]
                        let response = signup_poll::request(&signup_id).await;

                        #[cfg(feature = "ui-test-successful-signup")]
                        let response: Result<signup_poll::Response> = Ok(signup_poll::Response {
                            status: signup_poll::Status::Completed,
                            success: true,
                            error: None,
                        });

                        match response {
                            Ok(signup_poll::Response {
                                success: true,
                                error: None,
                                status: signup_poll::Status::Completed,
                            }) => {
                                tracing::info!("SIGNUP SUCCESS");
                                dd_incr!("main.count.http.user_enrollment.success.success_unique");
                                dd_incr!("main.count.http.user_enrollment.success.success");
                                return Status::Success;
                            }
                            Ok(signup_poll::Response {
                                success: false,
                                error: None,
                                status: signup_poll::Status::Completed,
                            }) => {
                                // This includes the following cases:
                                //   1. Backend duplicates
                                //   2. Backend legacy signup requests
                                //   3. Backend inflight matches
                                //   4. Backend detected fraud
                                //   5. Orb agent, internal, capture or pipeline failures
                                //   6. Orb detected fraud
                                tracing::info!("SIGNUP FAIL");
                                dd_incr!("main.count.http.user_enrollment.success.failed");
                                dd_incr!(
                                    "main.count.signup.result.failure.user_enrollment",
                                    "type:failure"
                                );
                                return Status::SignupVerificationNotSuccessful;
                            }
                            Ok(signup_poll::Response { error: Some(error), .. }) => {
                                tracing::error!("SIGNUP FAILURE: {}", error);
                                dd_incr!(
                                    "main.count.http.user_enrollment.error.server_error",
                                    "error_type:unknown"
                                );
                                dd_incr!(
                                    "main.count.signup.result.failure.user_enrollment",
                                    "type:server_failure",
                                    &format!("subtype:{}", error.to_lowercase())
                                );
                                return Status::ServerError;
                            }
                            Ok(signup_poll::Response {
                                status:
                                    signup_poll::Status::InProgress | signup_poll::Status::Accepted,
                                ..
                            }) => {
                                tracing::info!("SIGNUP IN PROGRESS");
                            }
                            Ok(signup_poll::Response {
                                status:
                                    status @ (signup_poll::Status::Error | signup_poll::Status::Failed),
                                ..
                            }) => {
                                tracing::error!("SIGNUP ERROR: {:?}", status);
                                dd_incr!(
                                    "main.count.http.user_enrollment.error.server_error",
                                    "error_type:status_error"
                                );
                                if matches!(status, signup_poll::Status::Failed) {
                                    dd_incr!(
                                        "main.count.signup.result.failure.user_enrollment",
                                        "type:server_failure",
                                        "subtype:failed"
                                    );
                                    return Status::ServerError;
                                }
                            }
                            Err(err) => {
                                tracing::error!("SIGNUP ERROR: {:?}", err);
                                dd_incr!(
                                    "main.count.http.user_enrollment.error.network_error",
                                    "error_type:poll"
                                );
                                if let Some(err_downcast) = err.downcast_ref::<reqwest::Error>() {
                                    if let Some(status) = err_downcast.status() {
                                        if status.is_client_error() {
                                            dd_incr!(
                                                "main.count.signup.result.failure.user_enrollment",
                                                "type:network_error",
                                                "subtype:poll_request"
                                            );
                                            log_decoding_error(&err);
                                            return Status::Error;
                                        }
                                    }
                                }
                                if i == POLL_STATUS_COUNT - 1 {
                                    dd_incr!(
                                        "main.count.signup.result.failure.user_enrollment",
                                        "type:network_error",
                                        "subtype:poll_request"
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(signup_post::Response {
                    software_version_status: signup_post::SoftwareVersionStatus::Blocked,
                }) => {
                    tracing::error!(
                        "SIGNUP ERROR: Orb component versions are outdated. Signups are blocked."
                    );
                    dd_incr!("main.count.http.user_enrollment.error.outdated", "error_type:normal");
                    return Status::SoftwareVersionOutdated;
                }
                Err(err) => {
                    tracing::error!("SIGNUP ERROR: {:?}", err);
                    dd_incr!(
                        "main.count.http.user_enrollment.error.network_error",
                        "error_type:normal"
                    );
                    if let Some(err_downcast) = err.downcast_ref::<reqwest::Error>() {
                        if let Some(status) = err_downcast.status() {
                            if status.is_client_error() {
                                dd_incr!(
                                    "main.count.signup.result.failure.user_enrollment",
                                    "type:network_error",
                                    "subtype:signup_request"
                                );
                                log_decoding_error(&err);
                                return Status::Error;
                            }
                        }
                    }
                    if i == RETRIES_COUNT - 1 {
                        dd_incr!(
                            "main.count.signup.result.failure.user_enrollment",
                            "type:network_error",
                            "subtype:signup_request"
                        );
                        log_decoding_error(&err);
                    }
                }
            }
        }
        dd_incr!("main.count.signup.result.failure.user_enrollment", "type:max_retry_exceeded");
        Status::Error
    }
}

fn make_signature(user_qr_code: &qr_scan::user::Data, pipeline: &Pipeline) -> Result<String> {
    let mut ctx = Context::new(&SHA256);
    ctx.update(ORB_ID.as_str().as_bytes());
    ctx.update(user_qr_code.user_id.as_bytes());
    ctx.update(pipeline.v2.ir_net_version.as_bytes());
    ctx.update(pipeline.v2.iris_version.as_bytes());
    ctx.update(pipeline.v2.eye_left.iris_code.as_bytes());
    ctx.update(pipeline.v2.eye_left.mask_code.as_bytes());
    ctx.update(pipeline.v2.eye_left.iris_code_version.as_bytes());
    ctx.update(pipeline.v2.eye_right.iris_code.as_bytes());
    ctx.update(pipeline.v2.eye_right.mask_code.as_bytes());
    ctx.update(pipeline.v2.eye_right.iris_code_version.as_bytes());
    let signed = secure_element::sign(ctx.finish())?;
    Ok(BASE64.encode(&signed))
}
