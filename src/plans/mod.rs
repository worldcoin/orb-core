//! Collection of plans.

use crate::{
    agents::image_notary::IdentificationImages,
    backend,
    backend::{s3_region, signup_post::SignupReason, upload_debug_report},
    brokers::Orb,
    calibration::Calibration,
    config::Config,
    consts::{
        BIOMETRIC_CAPTURE_TIMEOUT, DBUS_SIGNUP_OBJECT_PATH, DEFAULT_IR_LED_DURATION,
        DEFAULT_IR_LED_WAVELENGTH, EXTRA_IR_LED_WAVELENGTHS, IR_CAMERA_FRAME_RATE,
        QR_SCAN_INTERVAL, QR_SCAN_TIMEOUT,
    },
    dbus,
    debug_report::{self, DebugReport},
    inst_elapsed,
    led::QrScanSchema,
    logger::{LogOnError, DATADOG, NO_TAGS},
    mcu, network,
    sound::{self, Melody, Voice},
    sys_elapsed,
    utils::log_iris_data,
};
use eyre::{eyre, Error, Result};
use orb_wld_data_id::{ImageId, SignupId};
use qr_scan::{operator::DUMMY_OPERATOR_QR_CODE, user::DUMMY_USER_QR_CODE, Schema};
use std::time::{Duration, Instant, SystemTime};
use tokio::time::{self, sleep};

pub mod biometric_capture;
pub mod biometric_pipeline;
pub mod detect_face;
pub mod enroll_user;
pub mod fraud_check;
pub mod health_check;
pub mod idle;
pub mod qr_scan;
pub mod upload_self_custody_images;
pub mod warmup;
pub mod wifi;

/// High-level plan of the orb.
#[allow(clippy::struct_excessive_bools)]
pub struct MasterPlan {
    qr_scan_timeout: Duration,
    oneshot: bool,
    operator_qr_code_override: Option<qr_scan::operator::Data>,
    user_qr_code_override: Option<qr_scan::user::Data>,
    s3_region: orb_wld_data_id::S3Region,
    s3_region_str: String,
}

/// [`MasterPlan`] builder.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default, Clone)]
pub struct Builder {
    qr_scan_timeout: Option<Duration>,
    oneshot: bool,
    operator_qr_code_override: Option<qr_scan::operator::Data>,
    user_qr_code_override: Option<qr_scan::user::Data>,
    s3: Option<(orb_wld_data_id::S3Region, String)>,
}

impl Builder {
    /// Builds a new [`MasterPlan`].
    pub async fn build(self) -> Result<MasterPlan> {
        let Self { qr_scan_timeout, oneshot, operator_qr_code_override, user_qr_code_override, s3 } =
            self;
        let (s3_region, s3_region_str) = match s3 {
            Some(t) => t,
            None => s3_region::get_region().await?,
        };
        Ok(MasterPlan {
            qr_scan_timeout: qr_scan_timeout.unwrap_or(QR_SCAN_TIMEOUT),
            oneshot,
            operator_qr_code_override,
            user_qr_code_override,
            s3_region,
            s3_region_str,
        })
    }

    /// Sets the QR-code scan timeout.
    #[must_use]
    pub fn qr_scan_timeout(mut self, qr_scan_timeout: Duration) -> Self {
        self.qr_scan_timeout = Some(qr_scan_timeout);
        self
    }

    /// Sets the S3 region.
    #[must_use]
    pub fn s3(mut self, s3_region: orb_wld_data_id::S3Region, s3_region_str: String) -> Self {
        self.s3 = Some((s3_region, s3_region_str));
        self
    }

    /// Enables or disables exiting after the first successful signup.
    #[must_use]
    pub fn oneshot(mut self, oneshot: bool) -> Self {
        self.oneshot = oneshot;
        self
    }

    /// Sets the default operator QR-code value instead of asking the user.
    pub fn operator_qr_code(mut self, qr_code: Option<Option<&str>>) -> Result<Self> {
        self.operator_qr_code_override = qr_code
            .map(|qr_code| qr_code.unwrap_or(DUMMY_OPERATOR_QR_CODE))
            .map(|qr_code| {
                qr_scan::operator::Data::try_parse(qr_code)
                    .ok_or_else(|| eyre!("provide a valid operator QR code"))
            })
            .transpose()?;
        Ok(self)
    }

    /// Sets the default user QR-code value instead of asking the user.
    pub fn user_qr_code(mut self, qr_code: Option<Option<&str>>) -> Result<Self> {
        self.user_qr_code_override = qr_code
            .map(|qr_code| qr_code.unwrap_or(DUMMY_USER_QR_CODE))
            .map(|qr_code| {
                qr_scan::user::Data::try_parse(qr_code)
                    .ok_or_else(|| eyre!("provide a valid user QR code. We got {:?}", qr_code))
            })
            .transpose()?;
        Ok(self)
    }
}

impl MasterPlan {
    /// Returns a new [`Builder`].
    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Runs the high-level plan of the orb.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<bool> {
        let dbus = orb
            .dbus_conn
            .as_ref()
            .map(|conn| zbus::SignalContext::new(conn, DBUS_SIGNUP_OBJECT_PATH))
            .transpose()?;
        self.reset_hardware(orb, Duration::from_secs(10)).await?;
        loop {
            orb.led.idle();
            if !(self.oneshot) {
                // wait for button press to start next signup
                let start_signup = idle::Plan::default().run(orb).await?;
                if !start_signup {
                    break Ok(false);
                }
            }

            DATADOG.incr("orb.main.count.signup.during.general.signup_started", NO_TAGS).or_log();
            let mut debug_report = None;
            let success = Box::pin(self.do_signup(orb, &mut debug_report, dbus.as_ref())).await?;
            Box::pin(self.after_signup(orb, success, debug_report)).await?;

            orb.disable_image_notary();
            self.reset_hardware_except_led(orb).await?;
            if let Some(dbus_ctx) = dbus.as_ref() {
                crate::dbus::Signup::signup_finished(dbus_ctx, success).await?;
            }
            if self.oneshot {
                break Ok(true);
            }
            if !success {
                // wait for an extra button press to clear the failure
                // or clear after 30 seconds
                if let Ok(result) =
                    time::timeout(Duration::from_secs(30), idle::Plan::default().run(orb)).await
                {
                    result?;
                    orb.sound.build(sound::Type::Melody(Melody::StartIdle))?.push()?;
                }
            }
        }
    }

    async fn do_signup(
        &mut self,
        orb: &mut Orb,
        debug_report: &mut Option<debug_report::Builder>,
        dbus: Option<&zbus::SignalContext<'_>>,
    ) -> Result<bool> {
        let Some((capture_start, signup_id)) = self.start_signup(orb, dbus).await? else {
            return Ok(false);
        };
        let Some((operator_qr_code, user_qr_code, user_data)) = self.scan_qr_codes(orb).await?
        else {
            return Ok(false);
        };
        let debug_report = debug_report.insert(DebugReport::builder(
            capture_start,
            &signup_id,
            &operator_qr_code,
            &user_qr_code,
            &user_data,
        ));

        // wait for the sound to finish and user to get ready before starting the capture
        sleep(Duration::from_millis(1000)).await;

        let Some((capture, identification_image_ids)) =
            self.biometric_capture(orb, debug_report).await?
        else {
            return Ok(false);
        };
        let pipeline = self.biometric_pipeline(orb, debug_report, &capture).await?;
        let fraud_detected = self.detect_fraud(orb, debug_report, pipeline.as_ref()).await?;

        let signup_reason = if pipeline.is_none() {
            SignupReason::Failure
        } else if fraud_detected {
            SignupReason::Fraud
        } else {
            SignupReason::Normal
        };

        if orb.config.lock().await.upload_self_custody_images
            && !self
                .upload_self_custody_images(
                    orb,
                    debug_report,
                    &capture,
                    pipeline.as_ref(),
                    operator_qr_code,
                    user_qr_code,
                    user_data,
                    signup_reason,
                )
                .await?
        {
            return Ok(false);
        }
        Box::pin(self.enroll_user(
            orb,
            debug_report,
            identification_image_ids,
            &capture,
            pipeline.as_ref(),
            signup_reason,
        ))
        .await
    }

    async fn start_signup(
        &mut self,
        orb: &mut Orb,
        dbus: Option<&zbus::SignalContext<'_>>,
    ) -> Result<Option<(SystemTime, SignupId)>, Error> {
        orb.sound.build(sound::Type::Melody(Melody::StartSignup))?.push()?;
        orb.led.signup_start();
        let capture_start = SystemTime::now();
        if let Some(context) = &dbus {
            dbus::Signup::signup_started(context).await?;
        }
        let signup_id = SignupId::new(self.s3_region);
        tracing::info!("Starting signup with ID: {}", signup_id.to_string());
        Ok(Some((capture_start, signup_id)))
    }

    /// Sets the hardware to the idle state.
    pub async fn reset_hardware(&self, orb: &mut Orb, timeout: Duration) -> Result<()> {
        orb.disable_rgb_net();
        orb.disable_ir_net();
        orb.main_mcu.send(mcu::main::Input::VoltageRequestPeriod(10000)).await?;
        let future = async {
            self.reset_hardware_except_led(orb).await?;
            Ok(())
        };
        if let Ok(result) = time::timeout(timeout, future).await {
            result
        } else {
            tracing::error!("Hardware reset timed out");
            Ok(())
        }
    }

    /// Sets the hardware except UX LEDs to the idle state.
    pub async fn reset_hardware_except_led(&self, orb: &mut Orb) -> Result<()> {
        orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
        orb.disable_ir_led().await?;
        orb.main_mcu.send(mcu::main::Input::LiquidLens(None)).await?;
        Ok(())
    }

    /// Resets the mirror calibration.
    pub async fn reset_mirror_calibration(&self, orb: &mut Orb) -> Result<()> {
        let calibration = Calibration::default();
        calibration.store().await?;
        orb.enable_mirror()?;
        orb.recalibrate(calibration).await?;
        orb.disable_mirror();
        Ok(())
    }

    /// Resets the network and requests a new one.
    pub async fn reset_wifi_and_ensure_network(&self, orb: &mut Orb) -> Result<()> {
        network::reset().await?;
        wifi::Plan::new().ensure_network_connection(orb).await?;
        orb.reset_rgb_camera().await?;
        Ok(())
    }

    // TODO: I don't like that we have 3 code paths here. Refactor this with proper error rising and handling.
    async fn scan_qr_codes(
        &self,
        orb: &mut Orb,
    ) -> Result<Option<(qr_scan::user::Data, qr_scan::user::Data, backend::user_status::UserData)>>
    {
        let Some((operator_qr_code, duration_since_shot_ms)) =
            self.scan_operator_qr_code(orb).await?
        else {
            return Ok(None);
        };
        // a delay following the scan allows for a better user experience & increases the chance of
        // not reusing any previous RGB frame for the next QR-code scan
        if let Some(delay) =
            QR_SCAN_INTERVAL.checked_sub(Duration::from_millis(duration_since_shot_ms))
        {
            sleep(delay).await;
        }
        let Some((user_qr_code, user_data)) =
            self.scan_user_qr_code(orb, &operator_qr_code).await?
        else {
            return Ok(None);
        };
        Ok(Some((operator_qr_code, user_qr_code, user_data)))
    }

    /// Scans the operator QR-code.
    /// Returns the operator data and the duration of the HTTP request
    /// used to check the operator ID for consistent UX.
    /// An artificial delay is added before returning for better UX.
    async fn scan_operator_qr_code(
        &self,
        orb: &mut Orb,
    ) -> Result<Option<(qr_scan::user::Data, u64)>> {
        tracing::info!("Scanning operator QR-code");
        let qr_capture_start = Instant::now();
        loop {
            DATADOG
                .incr(
                    "orb.main.count.signup.during.general.distributor_identification_request",
                    NO_TAGS,
                )
                .or_log();

            let result = if let Some(new_timeout_ms) =
                self.qr_scan_timeout.checked_sub(qr_capture_start.elapsed())
            {
                if let Some(qr) = &self.operator_qr_code_override {
                    tracing::info!("Operator QR-code provided from CLI");
                    Ok(qr.clone())
                } else {
                    qr_scan::Plan::<qr_scan::operator::Data>::new(Some(new_timeout_ms))
                        .run(orb)
                        .await?
                }
            } else {
                Err(qr_scan::ScanError::Timeout)
            };
            orb.reset_rgb_camera().await?;
            let operator_qr_code = match result {
                Ok(qr_code) => {
                    DATADOG.incr("orb.main.count.global.distr_code_detected", NO_TAGS).or_log();
                    qr_code
                }
                Err(qr_scan::ScanError::Invalid) => {
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    orb.sound.build(sound::Type::Voice(Voice::WrongQrCodeFormat))?.push()?.await;
                    orb.led.qr_scan_unexpected(QrScanSchema::Operator);
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.distr_qr_code", [
                            "type:wrong_format",
                        ])
                        .or_log();
                    continue; // retry
                }
                Err(qr_scan::ScanError::Timeout) => {
                    orb.led.qr_scan_fail(QrScanSchema::Operator);
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    orb.sound.build(sound::Type::Voice(Voice::Timeout))?.push()?;
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.distr_qr_code", [
                            "type:timeout",
                        ])
                        .or_log();
                    tracing::error!("Timeout while scanning operator QR-code");
                    return Ok(None);
                }
            };

            orb.sound.build(sound::Type::Melody(Melody::QrCodeCapture))?.priority(2).push()?;

            match operator_qr_code {
                qr_scan::operator::Data::Normal(operator_qr_code) => {
                    if !check_signup_conditions(orb).await? {
                        return Ok(None);
                    }
                    tracing::info!("Operator QR-code detected: {operator_qr_code:?}");

                    if let Some(http_req_duration) = self
                        .verify_operator_qr_code(orb, &operator_qr_code, qr_capture_start)
                        .await?
                    {
                        return Ok(Some((operator_qr_code, http_req_duration)));
                    }
                }
                qr_scan::operator::Data::MagicResetMirror => {
                    tracing::info!("Magic QR-code detected: Reset Mirror");
                    let result = self.reset_mirror_calibration(orb).await;
                    if let Err(err) = result {
                        tracing::error!("Failed to reset mirror calibration: {err}");
                    }
                    orb.led.qr_scan_fail(QrScanSchema::Operator);
                    return Ok(None);
                }
                qr_scan::operator::Data::MagicResetWifi => {
                    tracing::info!("Magic QR-code detected: Reset Wi-Fi");
                    let result = self.reset_wifi_and_ensure_network(orb).await;
                    if let Err(err) = result {
                        tracing::error!("Failed to reset wifi: {err}");
                    }
                    orb.led.qr_scan_fail(QrScanSchema::Operator);
                    return Ok(None);
                }
            }
        }
    }

    /// Scans the user QR-code.
    #[allow(clippy::too_many_lines)]
    async fn scan_user_qr_code(
        &self,
        orb: &mut Orb,
        operator_qr_code: &qr_scan::user::Data,
    ) -> Result<Option<(qr_scan::user::Data, backend::user_status::UserData)>> {
        tracing::info!("Scanning user QR-code");
        DATADOG
            .incr("orb.main.count.signup.during.general.user_identification_request", NO_TAGS)
            .or_log();

        // QR capture starts now and timeout is updated after each scan attempt
        let qr_capture_start = Instant::now();
        loop {
            let result = if let Some(new_timeout_ms) =
                self.qr_scan_timeout.checked_sub(qr_capture_start.elapsed())
            {
                if let Some(qr) = &self.user_qr_code_override {
                    tracing::info!("User QR-code provided from CLI");
                    Ok(qr.clone())
                } else {
                    orb.reset_rgb_camera().await?;
                    qr_scan::Plan::<qr_scan::user::Data>::new(Some(new_timeout_ms)).run(orb).await?
                }
            } else {
                Err(qr_scan::ScanError::Timeout)
            };
            let user_qr_code = match result {
                Ok(user_qr_code) => {
                    DATADOG
                        .incr("orb.main.count.signup.during.general.user_qr_code_detected", NO_TAGS)
                        .or_log();
                    tracing::info!("User QR-code detected: {user_qr_code:?}");

                    // Filter out the operator QR code
                    if user_qr_code.user_id == operator_qr_code.user_id {
                        orb.led.qr_scan_unexpected(QrScanSchema::User);
                        tracing::info!(
                            "User QR-code is the same as the operator QR-code, retrying"
                        );
                        orb.sound
                            .build(sound::Type::Melody(Melody::SoundError))?
                            .priority(2)
                            .push()?;
                        // Give time to remove the QR code from the front of the camera
                        sleep(Duration::from_millis(1500)).await;
                        continue;
                    }
                    user_qr_code
                }
                Err(qr_scan::ScanError::Invalid) => {
                    orb.led.qr_scan_unexpected(QrScanSchema::User);
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    orb.sound.build(sound::Type::Voice(Voice::WrongQrCodeFormat))?.push()?.await;
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.user_qr_code", [
                            "type:wrong_format",
                        ])
                        .or_log();
                    tracing::error!("Invalid user QR-code format");
                    continue; // retry
                }
                Err(qr_scan::ScanError::Timeout) => {
                    orb.led.qr_scan_fail(QrScanSchema::User);
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    orb.sound.build(sound::Type::Voice(Voice::Timeout))?.push()?.await;
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.user_qr_code", ["type:timeout"])
                        .or_log();
                    tracing::error!("Timeout while scanning user QR-code");
                    return Ok(None);
                }
            };

            orb.sound.build(sound::Type::Melody(Melody::QrCodeCapture))?.priority(2).push()?;

            if let Some(user_data) =
                self.verify_user_qr_code(orb, &user_qr_code, qr_capture_start).await?
            {
                if orb.config.lock().await.upload_self_custody_images
                    && (user_data.self_custody_user_public_key.is_none()
                        || user_data.backend_iris_public_key.is_none()
                        || user_data.backend_iris_encrypted_private_key.is_none()
                        || user_data.backend_normalized_iris_public_key.is_none()
                        || user_data.backend_normalized_iris_encrypted_private_key.is_none()
                        || user_data.backend_face_public_key.is_none()
                        || user_data.backend_face_encrypted_private_key.is_none())
                {
                    orb.led.qr_scan_fail(QrScanSchema::User);
                    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.user_qr_code", [
                            "type:missing_public_keys",
                        ])
                        .or_log();
                    tracing::error!("User status error: missing one of required public keys");
                    continue; // retry
                }
                return Ok(Some((user_qr_code, user_data)));
            }
        }
    }

    /// Detects the user face.
    async fn detect_face(&self, orb: &mut Orb) -> Result<bool> {
        let t = Instant::now();
        let face_detected = detect_face::Plan::new().run(orb).await?;
        DATADOG.timing("orb.main.time.signup.face_detection", inst_elapsed!(t), NO_TAGS).or_log();
        if face_detected {
            tracing::info!("Face detected");
            DATADOG.incr("orb.main.count.signup.during.general.face_detected", NO_TAGS).or_log();
        } else {
            tracing::info!("Face not detected");
            DATADOG
                .incr("orb.main.count.signup.result.failure.face_detection", ["type:timeout"])
                .or_log();
            notify_failed_signup(orb, Some(Voice::FaceNotFound))?;
        }
        Ok(face_detected)
    }

    /// Performs the biometric capture.
    #[allow(clippy::too_many_lines)]
    async fn biometric_capture(
        &self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
    ) -> Result<Option<(biometric_capture::Capture, Option<IdentificationImages>)>> {
        if !self.detect_face(orb).await? {
            return Ok(None);
        }

        tracing::info!("Starting image saver");
        orb.start_image_notary(
            debug_report.signup_id.clone(),
            debug_report.user_data.data_policy.is_opt_in(),
        )
        .await?;
        let t = Instant::now();
        let mut wavelengths = vec![(DEFAULT_IR_LED_WAVELENGTH, DEFAULT_IR_LED_DURATION)];
        wavelengths.extend_from_slice(EXTRA_IR_LED_WAVELENGTHS);
        let plan = biometric_capture::Plan::new(
            &wavelengths,
            Some(BIOMETRIC_CAPTURE_TIMEOUT),
            &orb.config.lock().await.clone(),
        );
        let biometric_capture::Output { capture, log: bio_capture_log } = plan.run(orb).await?;
        DATADOG
            .timing("orb.main.time.signup.biometric_capture", inst_elapsed!(t), NO_TAGS)
            .or_log();
        tracing::info!("Stopping image saver");
        let capture_eyes = capture
            .as_ref()
            .filter(|_| debug_report.user_data.data_policy.is_opt_in())
            .map(|capture| {
                (
                    capture.eye_left.clone(),
                    capture.eye_right.clone(),
                    capture.face_self_custody_candidate.clone(),
                )
            });
        let (image_notary_log, identification_image_ids) =
            orb.stop_image_notary(capture_eyes).await?;

        debug_report.biometric_capture_history(bio_capture_log);
        debug_report.image_notary_history(image_notary_log);
        debug_report.identification_images(identification_image_ids.clone().unwrap_or_default());
        if let Some(capture) = capture {
            debug_report.rgb_net_metadata(
                capture.eye_left.rgb_net_estimate.clone(),
                capture.eye_right.rgb_net_estimate.clone(),
            );
            debug_report.biometric_capture_succeeded();
            orb.led.biometric_capture_success();
            if debug_report.user_data.data_policy.is_opt_in() {
                debug_report.biometric_capture_gps_location(
                    capture.latitude.unwrap_or(0.0),
                    capture.longitude.unwrap_or(0.0),
                );
            }
            orb.sound.build(sound::Type::Melody(Melody::IrisScanSuccess))?.cancel_all().push()?;
            Ok(Some((capture, identification_image_ids)))
        } else {
            tracing::error!("SIGNUP TIMEOUT");
            DATADOG
                .incr("orb.main.count.signup.result.failure.biometric_capture", ["type:timeout"])
                .or_log();
            notify_failed_signup(orb, Some(Voice::Timeout))?;
            Ok(None)
        }
    }

    /// Performs the biometric pipeline.
    #[allow(clippy::too_many_lines)]
    pub async fn biometric_pipeline(
        &mut self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        capture: &biometric_capture::Capture,
    ) -> Result<Option<biometric_pipeline::Pipeline>> {
        let pipeline = biometric_pipeline::Plan::new(capture)?.run(orb).await;
        let pipeline = match pipeline {
            Ok(pipeline) => pipeline,
            Err(e) => {
                if let Some(e) = e.downcast_ref::<biometric_pipeline::Error>() {
                    match e {
                        biometric_pipeline::Error::Timeout => {
                            tracing::error!("Biometric pipeline failed: timeout");
                            DATADOG
                                .incr("orb.main.count.signup.result.failure.biometric_pipeline", [
                                    "type:timeout",
                                ])
                                .or_log();
                        }
                        biometric_pipeline::Error::Agent => {
                            tracing::error!("Biometric pipeline failed: some agent failed");
                            DATADOG
                                .incr("orb.main.count.signup.result.failure.biometric_pipeline", [
                                    "type:agent",
                                ])
                                .or_log();
                            notify_failed_signup(orb, None)?;
                        }
                        biometric_pipeline::Error::Iris(error) => {
                            tracing::error!(
                                "Biometric pipeline failed due to iris agent: {}",
                                error
                            );
                            DATADOG
                                .incr("orb.main.count.signup.result.failure.biometric_pipeline", [
                                    "type:iris_agent",
                                ])
                                .or_log();
                            debug_report.iris_model_error(Some(error.clone()));
                            notify_failed_signup(orb, None)?;
                        }
                    }
                } else {
                    // In case we don't recognize the error,
                    // then it must come from a '?' in any
                    // biometric pipeline called method.
                    tracing::error!("Biometric pipeline failed: unknown error: {e:?}");
                    DATADOG
                        .incr("orb.main.count.signup.result.failure.biometric_pipeline", [
                            "type:unknown",
                        ])
                        .or_log();
                    notify_failed_signup(orb, None)?;
                };
                return Ok(None);
            }
        };

        debug_report.iris_model_metadata(
            pipeline.v2.eye_left.metadata.clone(),
            pipeline.v2.eye_right.metadata.clone(),
        );
        debug_report.iris_normalized_images(
            pipeline.v2.eye_left.iris_normalized_image.clone(),
            pipeline.v2.eye_right.iris_normalized_image.clone(),
        );
        debug_report.mega_agent_one_config(pipeline.mega_agent_one_config.clone());
        debug_report.mega_agent_two_config(pipeline.mega_agent_two_config.clone());
        debug_report.self_custody_bundle(pipeline.face_identifier_bundle.clone().ok());
        debug_report.self_custody_thumbnail(pipeline.face_identifier_bundle.clone().ok());
        log_iris_data(
            pipeline.v2.eye_left.iris_code.as_ref(),
            pipeline.v2.eye_left.mask_code.as_ref(),
            pipeline.v2.eye_left.iris_code_version.as_ref(),
            true,
            "master plan",
        );
        log_iris_data(
            pipeline.v2.eye_right.iris_code.as_ref(),
            pipeline.v2.eye_right.mask_code.as_ref(),
            pipeline.v2.eye_right.iris_code_version.as_ref(),
            false,
            "master plan",
        );
        orb.led.biometric_pipeline_success();
        Ok(Some(pipeline))
    }

    /// Performs the fraud checks.
    async fn detect_fraud(
        &mut self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        pipeline: Option<&biometric_pipeline::Pipeline>,
    ) -> Result<bool> {
        let Some(pipeline) = pipeline else {
            return Ok(false);
        };

        let t = Instant::now();
        let fcr = fraud_check::FraudChecks::new(pipeline).run();
        debug_report.fraud_check_report(fcr.clone());
        DATADOG.timing("orb.main.time.signup.fraud_checks", inst_elapsed!(t), NO_TAGS).or_log();

        if fcr.fraud_detected() {
            tracing::info!("Fraud check results {fcr:?}");
        }

        let config = &orb.config.lock().await.fraud_check_engine_config.clone();
        if fcr.fraud_detected_with_config(config) {
            tracing::warn!("FRAUD DETECTED - BLOCKING SIGNUP");
            return Ok(true);
        }
        Ok(false)
    }

    async fn enroll_user(
        &mut self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        identification_image_ids: Option<IdentificationImages>,
        capture: &biometric_capture::Capture,
        pipeline: Option<&biometric_pipeline::Pipeline>,
        signup_reason: SignupReason,
    ) -> Result<bool> {
        let t = Instant::now();
        let success = enroll_user::Plan {
            signup_id: debug_report.signup_id.clone(),
            operator_qr_code: debug_report.operator_qr_code.clone(),
            user_qr_code: debug_report.user_qr_code.clone(),
            user_data: debug_report.user_data.clone(),
            s3_region_str: self.s3_region_str.clone(),
            capture,
            pipeline,
            identification_image_ids: identification_image_ids.clone(),
            signup_reason,
        }
        .run(orb)
        .await?;
        DATADOG.timing("orb.main.time.signup.user_enrollment", inst_elapsed!(t), NO_TAGS).or_log();
        orb.led.biometric_pipeline_progress(1.0);

        if signup_reason == SignupReason::Failure {
            tracing::info!("User enrollment failed due to fraud");
            debug_report.signup_failure();
            orb.led.signup_fail();
            Ok(false)
        } else if signup_reason == SignupReason::Fraud {
            tracing::info!("User enrollment failed due to fraud");
            debug_report.signup_fraud();
            orb.led.signup_fail();
            Ok(false)
        } else if success {
            debug_report.signup_successful();
            orb.led.signup_unique();
            DATADOG
                .incr("orb.main.count.signup.result.success.successful_signup", NO_TAGS)
                .or_log();
            Ok(true)
        } else {
            tracing::info!("User enrollment failed");
            debug_report.signup_failure();
            orb.led.signup_fail();
            Ok(false)
        }
    }

    /// Uploads the signup data json and also trigger uploading of images, if user is opt-in, on failure
    /// or fraud.
    async fn upload_debug_report_and_opt_in_images(
        &self,
        orb: &mut Orb,
        mut debug_report: debug_report::Builder,
    ) -> Result<()> {
        let signup_id = debug_report.signup_id.clone();
        let is_opt_in = debug_report.user_data.data_policy.is_opt_in();

        // Upload images ONLY if the user is opt-in.
        if is_opt_in {
            // For all opt-in signups, irrespectively of success or failure, we upload the following images immediately.
            if orb.config.lock().await.upload_self_custody_thumbnail {
                match try_upload_self_custody_thumbnail(orb, signup_id.clone(), &debug_report).await
                {
                    Ok(Some(id)) => {
                        debug_report.insert_self_custody_thumbnail_id(id);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("Self-custody thumbnail image uploader failed with: {e}");
                    }
                };
            }
            if orb.config.lock().await.upload_iris_normalized_images {
                match try_upload_iris_normalized_images(orb, signup_id.clone(), &debug_report).await
                {
                    Ok(Some([left_image, left_mask, right_image, right_mask])) => {
                        debug_report.insert_iris_normalized_image_ids(
                            left_image,
                            left_mask,
                            right_image,
                            right_mask,
                        )?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("Iris normalized images uploader failed with: {e}");
                    }
                };
            }
        }

        let t1 = Instant::now();
        let debug_report = debug_report.build(SystemTime::now(), orb.config.lock().await.clone());
        match upload_debug_report::request(&signup_id, &debug_report).await {
            Ok(()) => {
                DATADOG
                    .incr("orb.main.count.data_collection.upload.success.signup_json", NO_TAGS)
                    .or_log();
            }
            Err(e) => {
                DATADOG
                    .incr("orb.main.count.data_collection.upload.error.signup_json", NO_TAGS)
                    .or_log();
                tracing::error!("Uploading signup data failed: {e}");
                // error_sound(&mut orb.sound, &err)?;
            }
        }
        DATADOG
            .timing("orb.main.time.signup.signup_json_upload", inst_elapsed!(t1), NO_TAGS)
            .or_log();

        Ok(())
    }

    async fn after_signup(
        &mut self,
        orb: &mut Orb,
        _success: bool,
        debug_report: Option<debug_report::Builder>,
    ) -> Result<()> {
        if let Some(debug_report) = debug_report {
            DATADOG
                .timing(
                    "orb.main.time.signup.full_signup",
                    sys_elapsed!(debug_report.start_timestamp),
                    NO_TAGS,
                )
                .or_log();

            Box::pin(self.upload_debug_report_and_opt_in_images(orb, debug_report)).await?;
        }
        Ok(())
    }

    /// Checks if `qr_code` is a valid operator QR-code through the backend.
    #[allow(clippy::cast_possible_truncation)]
    async fn verify_operator_qr_code(
        &self,
        orb: &mut Orb,
        qr_code: &qr_scan::user::Data,
        qr_capture_start: Instant,
    ) -> Result<Option<u64>> {
        let http_start = Instant::now();
        match backend::operator_status::request(qr_code).await {
            Ok(true) => {
                orb.sound.build(sound::Type::Melody(Melody::QrLoadSuccess))?.push()?;
                orb.led.qr_scan_success(QrScanSchema::Operator);
                DATADOG.incr("orb.main.count.global.distr_code_validated", NO_TAGS).or_log();
                tracing::info!("Operator QR-code validated: {qr_code:?}");
                DATADOG
                    .timing(
                        "orb.main.time.signup.distr_qr_code_capture",
                        inst_elapsed!(qr_capture_start),
                        NO_TAGS,
                    )
                    .or_log();
                return Ok(Some(http_start.elapsed().as_millis() as u64));
            }
            Ok(false) => {
                orb.led.qr_scan_fail(QrScanSchema::Operator);
                orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                orb.sound.build(sound::Type::Voice(Voice::QrCodeInvalid))?.push()?.await;
                DATADOG
                    .incr("orb.main.count.signup.result.failure.distr_qr_code", ["type:invalid_qr"])
                    .or_log();
            }
            Err(err) => {
                orb.led.qr_scan_fail(QrScanSchema::Operator);
                backend::error_sound(&mut *orb.sound, &err)?.await;
            }
        }
        Ok(None)
    }

    /// Checks if `qr_code` is a valid user QR-code through the backend.
    async fn verify_user_qr_code(
        &self,
        orb: &mut Orb,
        qr_code: &qr_scan::user::Data,
        qr_capture_start: Instant,
    ) -> Result<Option<backend::user_status::UserData>> {
        match backend::user_status::request(qr_code).await {
            Ok(Some(user_data)) => {
                orb.sound.build(sound::Type::Melody(Melody::UserQrLoadSuccess))?.push()?;
                orb.led.qr_scan_success(QrScanSchema::User);
                DATADOG
                    .incr("orb.main.count.signup.during.general.user_qr_code_validate", NO_TAGS)
                    .or_log();
                tracing::info!("User QR-code validated: {qr_code:?}");
                DATADOG
                    .timing(
                        "orb.main.time.signup.user_qr_code_capture",
                        inst_elapsed!(qr_capture_start),
                        NO_TAGS,
                    )
                    .or_log();
                return Ok(Some(user_data));
            }
            Ok(None) => {
                orb.led.qr_scan_fail(QrScanSchema::User);
                orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
                orb.sound.build(sound::Type::Voice(Voice::QrCodeInvalid))?.push()?.await;
                DATADOG
                    .incr("orb.main.count.signup.result.failure.user_qr_code", ["type:invalid_qr"])
                    .or_log();
            }
            Err(err) => {
                orb.led.qr_scan_fail(QrScanSchema::User);
                DATADOG
                    .incr("orb.main.count.signup.result.failure.user_qr_code", [
                        "type:validation_network_error",
                    ])
                    .or_log();
                backend::error_sound(&mut *orb.sound, &err)?.await;
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_self_custody_images(
        &self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        capture: &biometric_capture::Capture,
        pipeline: Option<&biometric_pipeline::Pipeline>,
        operator_qr_code: qr_scan::user::Data,
        user_qr_code: qr_scan::user::Data,
        user_data: backend::user_status::UserData,
        signup_reason: SignupReason,
    ) -> Result<bool> {
        let Some(pipeline) = pipeline else {
            return Ok(false);
        };

        macro_rules! data_error {
            ($field:literal) => {
                data_error!(
                    concat!("Image self-custody upload failed due to missing `", $field, "``"),
                    concat!("type:missing_", $field)
                )
            };
            ($message:expr, $dd_type:expr) => {
                tracing::error!($message);
                DATADOG
                    .incr("orb.main.count.signup.result.failure.upload_self_custody_images", [
                        $dd_type,
                    ])
                    .or_log();
                notify_failed_signup(orb, None)?;
                return Ok(false);
            };
        }
        if let Ok(bundle) = &pipeline.face_identifier_bundle {
            if let Some(error) = &bundle.error {
                data_error!(
                    "Face identifier bundle contains an error: {error:?}",
                    "type:face_identifier_bundle_error"
                );
            }
            if bundle.thumbnail.is_none() {
                data_error!("face_identifier_bundle.thumbnail");
            }
            if bundle.embeddings.is_none() {
                data_error!("face_identifier_bundle.embeddings");
            }
            if bundle.inference_backend.is_none() {
                data_error!("face_identifier_bundle.inference_backend");
            }
        } else {
            data_error!("face_identifier_bundle");
        }
        if pipeline.v2.eye_left.iris_normalized_image.is_none() {
            data_error!("v2.eye_left.iris_normalized_image");
        }
        if pipeline.v2.eye_right.iris_normalized_image.is_none() {
            data_error!("v2.eye_right.iris_normalized_image");
        }

        let t = Instant::now();
        orb.led.starting_enrollment();
        upload_self_custody_images::Plan {
            capture_start: debug_report.start_timestamp,
            signup_id: debug_report.signup_id.clone(),
            capture: capture.clone(),
            pipeline: pipeline.clone(),
            operator_qr_code,
            user_qr_code,
            user_data,
            signup_reason,
        }
        .run(orb)
        .await?;
        DATADOG
            .timing("orb.main.time.signup.upload_self_custody_images", inst_elapsed!(t), NO_TAGS)
            .or_log();
        Ok(true)
    }
}

/// Notify to operator & user that signup failed with LED, sound and optionally a voice
pub fn notify_failed_signup(orb: &mut Orb, voice: Option<Voice>) -> Result<()> {
    orb.led.signup_fail();
    orb.sound.build(sound::Type::Melody(Melody::SoundError))?.push()?;
    if let Some(voice) = voice {
        orb.sound.build(sound::Type::Voice(voice))?.push()?;
    }
    Ok(())
}

async fn try_upload_self_custody_thumbnail(
    orb: &mut Orb,
    signup_id: SignupId,
    debug_report: &debug_report::Builder,
) -> Result<Option<ImageId>> {
    if let Some(frame) = &debug_report.self_custody_thumbnail {
        orb.enable_image_uploader()?;
        let res = orb
            .image_uploader
            .enabled()
            .expect("image_uploader should be enabled")
            .upload_self_custody_thumbnail(signup_id, frame.clone())
            .await;
        orb.disable_image_uploader();
        res.map(Some)
    } else {
        Ok(None)
    }
}

async fn try_upload_iris_normalized_images(
    orb: &mut Orb,
    signup_id: SignupId,
    debug_report: &debug_report::Builder,
) -> Result<Option<[Option<ImageId>; 4]>> {
    if debug_report.left_iris_normalized_image.is_none()
        && debug_report.right_iris_normalized_image.is_none()
    {
        return Ok(None);
    }

    orb.enable_image_uploader()?;
    let res = orb
        .image_uploader
        .enabled()
        .expect("image_uploader should be enabled")
        .upload_iris_normalized_images(
            signup_id,
            debug_report.left_iris_normalized_image.clone(),
            debug_report.right_iris_normalized_image.clone(),
        )
        .await;
    orb.disable_image_uploader();
    Ok(Some(res?))
}

async fn check_signup_conditions(orb: &mut Orb) -> Result<bool> {
    if let Some(report) = orb.net_monitor.last_report()? {
        // Drop the mutex lock fast.
        let Config { block_signup_when_no_internet, .. } = *orb.config.lock().await;
        if block_signup_when_no_internet && report.is_no_internet() {
            orb.sound
                .build(sound::Type::Voice(Voice::InternetConnectionTooSlowToPerformSignups))?
                .max_delay(Duration::from_secs(2))
                .push()?;
            DATADOG
                .incr("orb.main.count.signup.result.failure.internet_check", [
                    "type:too_slow_to_start",
                ])
                .or_log();
            return Ok(false);
        }
        if report.is_slow_internet() {
            orb.sound
                .build(sound::Type::Voice(Voice::InternetConnectionTooSlowSignupsMightTakeLonger))?
                .max_delay(Duration::from_secs(2))
                .push()?;
            DATADOG
                .incr("orb.main.count.signup.result.failure.internet_check", [
                    "type:too_slow_to_start",
                ])
                .or_log();
            return Ok(true);
        }
    }
    Ok(true)
}
