//! User enrollment.

use super::{biometric_capture::Capture, biometric_pipeline::Pipeline, qr_scan};
use crate::{
    agents::image_notary::IdentificationImages,
    backend::{
        error_sound, signup_poll,
        signup_post::{self, SignupReason},
        user_status::UserData,
    },
    brokers::Orb,
    identification::ORB_ID,
    logger::{LogOnError, DATADOG, NO_TAGS},
    secure_element,
    sound::{self, Melody, Voice},
};
use data_encoding::BASE64;
use eyre::Result;
use orb_wld_data_id::SignupId;
use ring::digest::{Context, SHA256};
use std::time::Duration;
use tokio::{task, time::sleep};

const RETRIES_COUNT: usize = 3;
const POLL_STATUS_COUNT: usize = 30;
const POLL_STATUS_INTERVAL: Duration = Duration::from_secs(2);

/// User enrollment plan.
#[allow(missing_docs)]
pub struct Plan<'a> {
    pub signup_id: SignupId,
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub user_data: UserData,
    pub s3_region_str: String,
    pub capture: &'a Capture,
    pub pipeline: Option<&'a Pipeline>,
    pub identification_image_ids: Option<IdentificationImages>,
    pub signup_reason: SignupReason,
}

impl Plan<'_> {
    /// Runs the user enrollment plan.
    #[allow(clippy::too_many_lines)]
    pub async fn run(self, orb: &mut Orb) -> Result<bool> {
        let user_qr_code = self.user_qr_code.clone();
        let signature = if let Some(p) = self.pipeline.cloned() {
            match task::spawn_blocking(move || make_signature(&user_qr_code, &p)).await? {
                Ok(signature) => Some(signature),
                Err(err) => {
                    tracing::error!("Failed to calculate signature: {err:?}");
                    return Ok(false);
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
                &self.user_data,
                &self.s3_region_str,
                self.capture,
                self.pipeline,
                self.identification_image_ids.as_ref(),
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
                        orb.led.version_deprecated();
                        // orb.sound.build(sound::Type::Voice(Voice::VersionsDeprecated))?.push()?;
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
                        orb.led.version_deprecated();
                        #[cfg(not(feature = "stage"))]
                        {
                            orb.led.version_blocked();
                            orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                            return Ok(false);
                        }
                    }
                    for i in 0..POLL_STATUS_COUNT {
                        sleep(POLL_STATUS_INTERVAL).await;
                        let response = signup_poll::request(&signup_id).await;
                        #[cfg(feature = "ui-test")]
                        // fake signup response based on current time
                        let response: Result<signup_poll::Response> = Ok(signup_poll::Response {
                            status: signup_poll::Status::Completed,
                            unique: std::time::SystemTime::now()
                                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                                .unwrap_or(Duration::default())
                                .as_secs()
                                % 2
                                == 0,
                            error: None,
                        });
                        match response {
                            Ok(signup_poll::Response {
                                success: true,
                                error: None,
                                status: signup_poll::Status::Completed,
                            }) => {
                                tracing::info!("SIGNUP SUCCESS");
                                orb.sound
                                    .build(sound::Type::Melody(Melody::SignupSuccess))?
                                    .push()?;
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.success.\
                                         success_unique",
                                        NO_TAGS,
                                    )
                                    .or_log();
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.success.success",
                                        NO_TAGS,
                                    )
                                    .or_log();
                                return Ok(true);
                            }
                            Ok(signup_poll::Response {
                                success: false,
                                error: None,
                                status: signup_poll::Status::Completed,
                            }) => {
                                tracing::info!("SIGNUP FAIL");
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.success.\
                                         failed_duplicate",
                                        NO_TAGS,
                                    )
                                    .or_log();
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.success.failed",
                                        NO_TAGS,
                                    )
                                    .or_log();
                                DATADOG
                                    .incr("orb.main.count.signup.result.failure.user_enrollment", [
                                        "type:duplicate",
                                    ])
                                    .or_log();
                                DATADOG
                                    .incr("orb.main.count.signup.result.failure.user_enrollment", [
                                        "type:failure",
                                    ])
                                    .or_log();
                                orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                                orb.sound
                                    .build(sound::Type::Voice(
                                        Voice::VerificationNotSuccessfulPleaseTryAgain,
                                    ))?
                                    .push()?;
                                return Ok(false);
                            }
                            Ok(signup_poll::Response { error: Some(error), .. }) => {
                                tracing::error!("SIGNUP FAILURE: {}", error);
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.error.server_error",
                                        ["error_type:unknown"],
                                    )
                                    .or_log();
                                DATADOG
                                    .incr("orb.main.count.signup.result.failure.user_enrollment", [
                                        "type:server_failure".to_string(),
                                        format!("subtype:{}", error.to_lowercase()),
                                    ])
                                    .or_log();
                                orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                                orb.sound.build(sound::Type::Voice(Voice::ServerError))?.push()?;
                                return Ok(false);
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
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.error.server_error",
                                        ["error_type:status_error"],
                                    )
                                    .or_log();
                                if matches!(status, signup_poll::Status::Failed) {
                                    DATADOG
                                        .incr(
                                            "orb.main.count.signup.result.failure.user_enrollment",
                                            ["type:server_failure", "subtype:failed"],
                                        )
                                        .or_log();
                                    orb.sound
                                        .build(sound::Type::Melody(Melody::SoundError))?
                                        .push()?;
                                    orb.sound
                                        .build(sound::Type::Voice(Voice::ServerError))?
                                        .push()?;
                                    return Ok(false);
                                }
                            }
                            Err(err) => {
                                tracing::error!("SIGNUP ERROR: {:?}", err);
                                DATADOG
                                    .incr(
                                        "orb.main.count.http.user_enrollment.error.network_error",
                                        ["error_type:poll"],
                                    )
                                    .or_log();
                                if let Some(err_downcast) = err.downcast_ref::<reqwest::Error>() {
                                    if let Some(status) = err_downcast.status() {
                                        if status.is_client_error() {
                                            DATADOG
                                                .incr(
                                                    "orb.main.count.signup.result.failure.\
                                                     user_enrollment",
                                                    ["type:network_error", "subtype:poll_request"],
                                                )
                                                .or_log();
                                            error_sound(&mut *orb.sound, &err)?;

                                            return Ok(false);
                                        }
                                    }
                                }
                                if i == POLL_STATUS_COUNT - 1 {
                                    DATADOG
                                        .incr(
                                            "orb.main.count.signup.result.failure.user_enrollment",
                                            ["type:network_error", "subtype:poll_request"],
                                        )
                                        .or_log();
                                    error_sound(&mut *orb.sound, &err)?;
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
                    DATADOG
                        .incr("orb.main.count.http.user_enrollment.error.outdated", [
                            "error_type:normal"
                        ])
                        .or_log();
                    orb.led.version_blocked();
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    return Ok(false);
                }
                Err(err) => {
                    tracing::error!("SIGNUP ERROR: {:?}", err);
                    DATADOG
                        .incr("orb.main.count.http.user_enrollment.error.network_error", [
                            "error_type:normal",
                        ])
                        .or_log();
                    if let Some(err_downcast) = err.downcast_ref::<reqwest::Error>() {
                        if let Some(status) = err_downcast.status() {
                            if status.is_client_error() {
                                DATADOG
                                    .incr("orb.main.count.signup.result.failure.user_enrollment", [
                                        "type:network_error",
                                        "subtype:signup_request",
                                    ])
                                    .or_log();
                                error_sound(&mut *orb.sound, &err)?;
                                return Ok(false);
                            }
                        }
                    }
                    if i == RETRIES_COUNT - 1 {
                        DATADOG
                            .incr("orb.main.count.signup.result.failure.user_enrollment", [
                                "type:network_error",
                                "subtype:signup_request",
                            ])
                            .or_log();
                        error_sound(&mut *orb.sound, &err)?;
                    }
                }
            }
        }
        DATADOG
            .incr("orb.main.count.signup.result.failure.user_enrollment", [
                "type:max_retry_exceeded",
            ])
            .or_log();
        Ok(false)
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
