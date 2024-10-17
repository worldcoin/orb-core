//! Collection of plans.

use self::personal_custody_package::PersonalCustodyPackages;
#[cfg(feature = "livestream")]
use crate::agents::livestream;
use crate::{
    agents::{camera::Frame, data_uploader, image_notary::IdentificationImages},
    backend::{
        self,
        endpoints::RELAY_BACKEND_URL,
        operator_status::Coordinates,
        orb_os_status::{self, OrbOsVersionStatus},
        s3_region,
        signup_post::SignupReason,
        upload_debug_report,
    },
    brokers::Orb,
    calibration::Calibration,
    config::Config,
    consts::{
        BIOMETRIC_CAPTURE_TIMEOUT, CALIBRATION_FILE_PATH, DBUS_SIGNUP_OBJECT_PATH,
        DEFAULT_IR_LED_DURATION, DEFAULT_IR_LED_WAVELENGTH, DETECT_FACE_TIMEOUT,
        DETECT_FACE_TIMEOUT_SELF_SERVE, EXTRA_IR_LED_WAVELENGTHS, IR_CAMERA_FRAME_RATE,
        QR_SCAN_INTERVAL, QR_SCAN_TIMEOUT,
    },
    dbus, dd_incr, dd_timing,
    debug_report::{self, DebugReport, SignupStatus},
    identification::{self, get_orb_token, ORB_ID},
    mcu, network,
    ui::{QrScanSchema, QrScanUnexpectedReason, SignupFailReason},
    utils::log_iris_data,
};
use agentwire::port;
use eyre::{eyre, Error, Result};
use futures::SinkExt;
use orb_relay_client::client::Client;
use orb_relay_messages::{common, self_serve};
use orb_wld_data_id::SignupId;
use qr_scan::{
    operator::DUMMY_OPERATOR_QR_CODE,
    user::{SignupExtensionConfig, DUMMY_USER_QR_CODE},
    Schema,
};
use ring::digest::Digest;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::time::{self, sleep};
use walkdir::WalkDir;

#[cfg(feature = "allow-plan-mods")]
mod _imports_for_plan_mods {
    pub use crate::{
        agents::{
            camera,
            python::{self, init_sys_argv, ir_net, rgb_net},
        },
        backend::signup_post,
        plans::biometric_capture::SelfCustodyCandidate,
    };
    pub use pyo3::Python;
    pub use std::fs::File;
    pub use tokio::{fs, task::spawn_blocking};
}
#[cfg(feature = "allow-plan-mods")]
#[allow(clippy::wildcard_imports)]
use self::_imports_for_plan_mods::*;

pub mod biometric_capture;
pub mod biometric_pipeline;
pub mod detect_face;
pub mod enroll_user;
pub mod fraud_check;
pub mod health_check;
pub mod idle;
#[cfg(feature = "integration_testing")]
pub mod integration_testing;
pub mod personal_custody_package;
pub mod qr_scan;
pub mod warmup;
pub mod wifi;

/// High-level plan of the orb.
#[allow(clippy::struct_excessive_bools)]
pub struct MasterPlan {
    qr_scan_timeout: Duration,
    oneshot: bool,
    #[cfg(feature = "allow-plan-mods")]
    skip_pipeline: bool,
    #[cfg(feature = "allow-plan-mods")]
    skip_fraud_checks: bool,
    #[cfg(feature = "allow-plan-mods")]
    biometric_input: Option<PathBuf>,
    operator_qr_code_override: Option<qr_scan::operator::Data>,
    user_qr_code_override: Option<(qr_scan::user::Data, String)>,
    s3_region: orb_wld_data_id::S3Region,
    s3_region_str: String,
    ui_idle_delay: Option<time::Sleep>,
    #[cfg(feature = "integration_testing")]
    ci_hacks: Option<integration_testing::CiHacks>,
    #[cfg(feature = "internal-data-acquisition")]
    data_acquisition: bool,
    signup_flag: Arc<AtomicBool>,
}

/// [`MasterPlan`] builder.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default, Clone)]
pub struct Builder {
    qr_scan_timeout: Option<Duration>,
    oneshot: bool,
    #[cfg(feature = "allow-plan-mods")]
    skip_pipeline: bool,
    #[cfg(feature = "allow-plan-mods")]
    skip_fraud_checks: bool,
    #[cfg(feature = "allow-plan-mods")]
    biometric_input: Option<PathBuf>,
    operator_qr_code_override: Option<qr_scan::operator::Data>,
    user_qr_code_override: Option<(qr_scan::user::Data, String)>,
    s3: Option<(orb_wld_data_id::S3Region, String)>,
    #[cfg(feature = "integration_testing")]
    ci_hacks: Option<integration_testing::CiHacks>,
    #[cfg(feature = "internal-data-acquisition")]
    data_acquisition: bool,
    signup_flag: Option<Arc<AtomicBool>>,
}

/// Helper struct to hold the resolved QR codes.
#[derive(Clone)]
pub struct OperatorData {
    /// The operator's QR code.
    pub qr_code: qr_scan::user::Data,
    /// The operator's location data.
    pub location_data: backend::operator_status::LocationData,
    /// The time when the QR code was scanned.
    pub timestamp: Instant,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
enum QrCodes {
    Both {
        operator_data: OperatorData,
        user_qr_code: qr_scan::user::Data,
        user_data: backend::user_status::UserData,
        user_qr_code_string: String,
    },
    Operator {
        operator_data: OperatorData,
    },
    None,
}

/// Helper struct to hold the resolved QR codes.
pub struct ResolvedQrCodes {
    /// Operator data (QR code + location data).
    pub operator_data: OperatorData,
    /// User QR code.
    pub user_qr_code: qr_scan::user::Data,
    /// User data.
    pub user_data: backend::user_status::UserData,
    /// User QR code string.
    pub user_qr_code_string: String,
}

struct SignupResult {
    success: bool,
    capture_start: SystemTime,
    signup_id: SignupId,
    debug_report: Option<debug_report::Builder>,
}

impl Builder {
    /// Builds a new [`MasterPlan`].
    pub async fn build(self) -> Result<MasterPlan> {
        let Self {
            qr_scan_timeout,
            oneshot,
            #[cfg(feature = "allow-plan-mods")]
            skip_pipeline,
            #[cfg(feature = "allow-plan-mods")]
            skip_fraud_checks,
            #[cfg(feature = "allow-plan-mods")]
            biometric_input,
            operator_qr_code_override,
            user_qr_code_override,
            s3,
            #[cfg(feature = "integration_testing")]
            ci_hacks,
            #[cfg(feature = "internal-data-acquisition")]
            data_acquisition,
            signup_flag,
        } = self;
        let (s3_region, s3_region_str) = match s3 {
            Some(t) => t,
            None => s3_region::get_region().await?,
        };
        Ok(MasterPlan {
            qr_scan_timeout: qr_scan_timeout.unwrap_or(QR_SCAN_TIMEOUT),
            oneshot,
            #[cfg(feature = "allow-plan-mods")]
            skip_pipeline,
            #[cfg(feature = "allow-plan-mods")]
            skip_fraud_checks,
            #[cfg(feature = "allow-plan-mods")]
            biometric_input,
            operator_qr_code_override,
            user_qr_code_override,
            s3_region,
            s3_region_str,
            ui_idle_delay: None,
            #[cfg(feature = "integration_testing")]
            ci_hacks,
            #[cfg(feature = "internal-data-acquisition")]
            data_acquisition,
            signup_flag: signup_flag.unwrap_or_default(),
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

    /// Enables or disables skipping biometric pipeline.
    #[cfg(feature = "allow-plan-mods")]
    #[must_use]
    pub fn skip_pipeline(mut self, skip_pipeline: bool) -> Self {
        self.skip_pipeline = skip_pipeline;
        self
    }

    /// Enables or disables skipping fraud checks.
    #[cfg(feature = "allow-plan-mods")]
    #[must_use]
    pub fn skip_fraud_checks(mut self, skip_fraud_checks: bool) -> Self {
        self.skip_fraud_checks = skip_fraud_checks;
        self
    }

    /// Sets a path to a directory with biometric data instead of running
    /// biometric capture.
    #[cfg(feature = "allow-plan-mods")]
    #[must_use]
    pub fn biometric_input(mut self, biometric_input: Option<PathBuf>) -> Self {
        self.biometric_input = biometric_input;
        self
    }

    /// Enable CI hacks.
    #[cfg(feature = "integration_testing")]
    #[must_use]
    pub fn ci_hacks(mut self, ci_hacks: Option<integration_testing::CiHacks>) -> Self {
        self.ci_hacks = ci_hacks;
        self
    }

    /// Enables data acquisition mode.
    #[cfg(feature = "internal-data-acquisition")]
    #[must_use]
    pub fn data_acquisition(mut self, data_acquisition: bool) -> Self {
        self.data_acquisition = data_acquisition;
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
                    .map(|x| (x, qr_code.to_string()))
            })
            .transpose()?;
        Ok(self)
    }

    /// Sets the biometric capture state atomic flag.
    #[must_use]
    pub fn signup_flag(mut self, signup_flag: Arc<AtomicBool>) -> Self {
        self.signup_flag = Some(signup_flag);
        self
    }
}

impl MasterPlan {
    /// Returns a new [`Builder`].
    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Runs the high-level plan of the orb.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<()> {
        let Config {
            self_serve,
            self_serve_button,
            orb_relay_shutdown_wait_for_pending_messages,
            orb_relay_shutdown_wait_for_shutdown,
            operator_qr_expiration_time,
            ..
        } = *orb.config.lock().await;
        let dbus = orb
            .dbus_conn
            .as_ref()
            .map(|conn| zbus::SignalContext::new(conn, DBUS_SIGNUP_OBJECT_PATH))
            .transpose()?;
        self.reset_hardware(orb, Duration::from_secs(10)).await?;
        orb.enable_data_uploader()?;
        let mut initial_qr_codes = QrCodes::None;
        loop {
            self.scan_initial_qr_codes(
                orb,
                &mut initial_qr_codes,
                self_serve,
                operator_qr_expiration_time,
            )
            .await?;
            let Some(qr_codes) = self
                .idle_wait_for_signup_request(
                    orb,
                    &initial_qr_codes,
                    self_serve,
                    self_serve_button,
                    operator_qr_expiration_time,
                )
                .await?
            else {
                continue;
            };

            dd_incr!("main.count.signup.during.general.signup_started");
            self.signup_flag.store(true, Ordering::Relaxed);
            let signup_result = Box::pin(self.do_signup(orb, qr_codes, dbus.as_ref())).await?;
            let success = signup_result.success;
            Box::pin(self.after_signup(orb, signup_result)).await?;
            self.signup_flag.store(false, Ordering::Relaxed);

            orb.disable_image_notary();
            if let Some(r) = orb.orb_relay.as_mut() {
                r.graceful_shutdown(
                    orb_relay_shutdown_wait_for_pending_messages,
                    orb_relay_shutdown_wait_for_shutdown,
                )
                .await;
            }
            orb.orb_relay = None;
            self.reset_hardware_except_led(orb).await?;
            if let Some(dbus_ctx) = dbus.as_ref() {
                dbus::Signup::signup_finished(dbus_ctx, success).await?;
            }

            if self.oneshot || self.has_biometric_input() {
                break Ok(());
            }
            self.ui_idle_delay = Some(time::sleep(Duration::from_secs(10)));
        }
    }

    async fn idle_wait_for_signup_request(
        &mut self,
        orb: &mut Orb,
        qr_codes: &QrCodes,
        self_serve: bool,
        self_serve_button: bool,
        operator_qr_expiration_time: Duration,
    ) -> Result<Option<QrCodes>> {
        // We currently support 4 scenarios:
        // 1. Internal testing with a biometric input file.
        // 2. Self-serve mode that always scans for a user QR code.
        // 3. Self-serve mode that expects a button press to ask for a user QR code.
        // 4. Normal mode that expects a button press to ask for an operator QR code and then a user QR code.
        //
        // Scenarios 3 and 4 are handled by the same code path in the following last else-statement.
        let ui_idle_delay = self.ui_idle_delay.take();
        let qr_codes = if self.oneshot || self.has_biometric_input() {
            qr_codes.clone()
        } else if self_serve && !self_serve_button {
            orb.set_phase("User QR-code idle scanning").await;
            let QrCodes::Operator { operator_data } = &qr_codes else {
                panic!("operator QR code needs to be scanned beforehand in self-serve mode");
            };
            let Some((user_qr_code, user_data, user_qr_code_string)) = self
                .idle_scan_user_qr_code(
                    orb,
                    operator_data,
                    operator_qr_expiration_time,
                    ui_idle_delay,
                )
                .await?
            else {
                return Ok(None);
            };
            qr_codes.with_user_qr_code(user_qr_code, user_data, user_qr_code_string)
        } else {
            orb.set_phase("Idle waiting for button press").await;
            self.idle_wait_for_button_press(orb, ui_idle_delay).await?;
            orb.ui.signup_start_operator();
            qr_codes.clone()
        };
        Ok(Some(qr_codes))
    }

    async fn idle_wait_for_button_press(
        &mut self,
        orb: &mut Orb,
        ui_idle_delay: Option<time::Sleep>,
    ) -> Result<()> {
        match idle::Plan::new(
            ui_idle_delay,
            #[cfg(feature = "internal-data-acquisition")]
            self.data_acquisition,
        )
        .run(orb)
        .await?
        {
            idle::Value::UserQrCode(_) | idle::Value::TimedOut => unreachable!(),
            idle::Value::ButtonPress => Ok(()),
        }
    }

    async fn idle_scan_user_qr_code(
        &mut self,
        orb: &mut Orb,
        operator_data: &OperatorData,
        operator_qr_expiration_time: Duration,
        mut ui_idle_delay: Option<time::Sleep>,
    ) -> Result<Option<(qr_scan::user::Data, backend::user_status::UserData, String)>> {
        loop {
            orb.reset_rgb_camera().await?;
            match idle::Plan::with_user_qr_scan(
                ui_idle_delay.take(),
                Some(operator_qr_expiration_time.saturating_sub(operator_data.timestamp.elapsed())),
                #[cfg(feature = "internal-data-acquisition")]
                self.data_acquisition,
            )
            .run(orb)
            .await?
            {
                idle::Value::UserQrCode(qr_scan_result) => {
                    if !check_signup_conditions(orb).await? {
                        continue;
                    }
                    if let Some(Some((user_qr_code, user_data, user_qr_code_string))) =
                        self.handle_user_qr_code(qr_scan_result, orb, operator_data, None).await?
                    {
                        break Ok(Some((user_qr_code, user_data, user_qr_code_string)));
                    }
                }
                idle::Value::TimedOut => break Ok(None),
                idle::Value::ButtonPress => unreachable!(),
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn do_signup(
        &mut self,
        orb: &mut Orb,
        qr_codes: QrCodes,
        dbus: Option<&zbus::SignalContext<'_>>,
    ) -> Result<SignupResult> {
        let Config {
            self_serve,
            pcp_v3,
            orb_relay_announce_orb_id_retries,
            orb_relay_announce_orb_id_timeout,
            orb_relay_shutdown_wait_for_pending_messages,
            orb_relay_shutdown_wait_for_shutdown,
            operator_qr_expiration_time,
            ..
        } = *orb.config.lock().await;
        let mut result = self.start_signup(orb, dbus).await?;
        let Some(qr_codes) =
            self.scan_remaining_qr_codes(orb, qr_codes, operator_qr_expiration_time).await?
        else {
            return Ok(result);
        };
        let debug_report = result.debug_report.insert(DebugReport::builder(
            result.capture_start,
            &result.signup_id,
            &qr_codes,
            orb.config.lock().await.clone(),
        ));

        if !self.is_orb_os_version_allowed(debug_report).await {
            #[cfg(feature = "stage")]
            notify_failed_signup(orb, Some(SignupFailReason::SoftwareVersionBlocked));
            #[cfg(not(feature = "stage"))]
            return Ok(result);
        }

        if self_serve && qr_codes.user_data.orb_relay_app_id.is_none() {
            tracing::error!("Self-serve: orb_relay_app_id is missing in the user data");
            debug_report.signup_app_incompatible_failure();
            return Ok(result);
        }
        if let Some(orb_relay_app_id) = &qr_codes.user_data.orb_relay_app_id {
            if let Err(e) = orb_relay_announce_orb_id(
                orb,
                orb_relay_app_id.clone(),
                self_serve,
                orb_relay_announce_orb_id_retries,
                orb_relay_announce_orb_id_timeout,
                orb_relay_shutdown_wait_for_pending_messages,
                orb_relay_shutdown_wait_for_shutdown,
            )
            .await
            {
                tracing::error!("{e}");
                debug_report.signup_orb_relay_failure();
                return Ok(result);
            }
        }

        // wait for the sound to finish and user to get ready before starting the capture
        sleep(Duration::from_millis(3000)).await;

        let capture = self.biometric_capture(orb, debug_report).await?;
        self.after_biometric_capture(orb, debug_report, capture.is_some(), self_serve).await?;
        let Some(capture) = capture else {
            return Ok(result);
        };
        if self.skip_pipeline() || debug_report.signup_extension_config.is_some() {
            result.success = true;
            return Ok(result);
        }
        let pipeline = Box::pin(self.biometric_pipeline(orb, debug_report, &capture)).await?;
        let fraud_detected = !self.skip_fraud_checks()
            && self.detect_fraud(orb, debug_report, pipeline.as_ref()).await?;
        let signup_reason = if pipeline.is_none() {
            SignupReason::Failure
        } else if fraud_detected {
            SignupReason::Fraud
        } else {
            SignupReason::Normal
        };
        let user_id = qr_codes.user_qr_code.user_id.clone();
        let user_centric_signup = qr_codes.user_data.user_centric_signup;
        if let Ok(mut credentials) = qr_codes.try_into() {
            let personal_custody_package::Credentials { pcp_version, .. } = &mut credentials;
            if !pcp_v3 {
                *pcp_version = 2;
            }
            let pcp_version = *pcp_version;
            let packages = match Box::pin(self.build_pcp(
                orb,
                credentials,
                &capture,
                pipeline.as_ref(),
                debug_report,
                signup_reason,
            ))
            .await
            {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return Ok(result);
                }
                Err(e) => {
                    tracing::error!("{e}");
                    return Ok(result);
                }
            };
            data_uploader::wait_queues(orb.data_uploader.enabled().unwrap()).await?;
            if !self
                .upload_pcp_tier_0(
                    orb,
                    &result.signup_id,
                    &user_id,
                    packages.tier0,
                    packages.tier0_checksum,
                    if pcp_version >= 3 { Some(0) } else { None },
                )
                .await?
            {
                return Ok(result);
            }
            if pcp_version >= 3 {
                orb.data_uploader
                    .enabled()
                    .unwrap()
                    .send(port::Input::new(data_uploader::Input::Pcp(data_uploader::Pcp {
                        signup_id: result.signup_id.clone(),
                        user_id: user_id.clone(),
                        data: packages.tier1,
                        checksum: packages.tier1_checksum.as_ref().to_vec(),
                        tier: 1,
                    })))
                    .await?;
                orb.data_uploader
                    .enabled()
                    .unwrap()
                    .send(port::Input::new(data_uploader::Input::Pcp(data_uploader::Pcp {
                        signup_id: result.signup_id.clone(),
                        user_id,
                        data: packages.tier2,
                        checksum: packages.tier2_checksum.as_ref().to_vec(),
                        tier: 2,
                    })))
                    .await?;
            }
        }

        let success = if user_centric_signup && !orb.config.lock().await.ignore_user_centric_signups
        {
            debug_report.enrollment_status(match signup_reason {
                SignupReason::Normal => enroll_user::Status::Success,
                _ => enroll_user::Status::Error,
            });
            signup_reason == SignupReason::Normal
        } else {
            Box::pin(self.enroll_user(
                orb,
                debug_report,
                &capture,
                pipeline.as_ref(),
                signup_reason,
            ))
            .await
            .is_success()
        };

        Self::report_signup_reason(success, signup_reason, debug_report);

        result.success =
            debug_report.enrollment_status.as_ref().map_or(false, enroll_user::Status::is_success);
        Ok(result)
    }

    fn report_signup_reason(
        success: bool,
        signup_reason: SignupReason,
        debug_report: &mut debug_report::Builder,
    ) {
        if signup_reason == SignupReason::Failure {
            tracing::info!("User enrollment failed due to a failure in the pipeline");
            debug_report.signup_orb_failure();
        } else if signup_reason == SignupReason::Fraud {
            tracing::info!("User enrollment failed due to fraud");
            debug_report.signup_fraud();
        } else if success {
            debug_report.signup_successful();
            dd_incr!("main.count.signup.result.success.successful_signup");
        } else {
            tracing::info!("User enrollment failed");
            debug_report.signup_server_failure();
        }
    }

    #[allow(unused_variables)]
    async fn start_signup(
        &mut self,
        orb: &mut Orb,
        dbus: Option<&zbus::SignalContext<'_>>,
    ) -> Result<SignupResult, Error> {
        let capture_start = SystemTime::now();
        if let Some(context) = dbus {
            dbus::Signup::signup_started(context).await?;
        }
        let signup_id = SignupId::new(self.s3_region);
        tracing::info!("Starting signup with ID: {}", signup_id.to_string());
        #[cfg(feature = "livestream")]
        if let Some(livestream) = orb.livestream.enabled() {
            livestream.send(port::Input::new(livestream::Input::Clear)).await?;
        }
        let signup_result =
            SignupResult { success: false, capture_start, signup_id, debug_report: None };
        Ok(signup_result)
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
        let calibration: Calibration = (&*orb.config.lock().await).into();
        calibration.store(CALIBRATION_FILE_PATH).await?;
        orb.enable_mirror()?;
        orb.recalibrate(calibration).await?;
        orb.disable_mirror();
        Ok(())
    }

    /// Resets the network and requests a new one.
    pub async fn reset_wifi_and_ensure_network(&self, orb: &mut Orb) -> Result<()> {
        network::reset().await?;
        wifi::Plan.ensure_network_connection(orb).await?;
        orb.reset_rgb_camera().await?;
        Ok(())
    }

    async fn scan_initial_qr_codes(
        &mut self,
        orb: &mut Orb,
        qr_codes: &mut QrCodes,
        self_serve: bool,
        operator_qr_expiration_time: Duration,
    ) -> Result<()> {
        if self_serve
            && qr_codes
                .operator_timestamp()
                .map_or(true, |ts| ts.elapsed() > operator_qr_expiration_time)
        {
            loop {
                let qr_capture_start = Instant::now();
                let operator_qr_code =
                    self.scan_operator_qr_code(orb, None).await?.expect("to never timeout");
                let Some(operator_qr_code) =
                    self.handle_magic_operator_qr_code(orb, operator_qr_code).await?
                else {
                    continue;
                };
                let Some((_, operator_location_data)) =
                    self.verify_operator_qr_code(orb, &operator_qr_code, qr_capture_start).await?
                else {
                    continue;
                };
                *qr_codes = QrCodes::Operator {
                    operator_data: OperatorData {
                        qr_code: operator_qr_code,
                        location_data: operator_location_data,
                        timestamp: Instant::now(),
                    },
                };
                break;
            }
        }
        Ok(())
    }

    async fn scan_remaining_qr_codes(
        &mut self,
        orb: &mut Orb,
        qr_codes: QrCodes,
        operator_qr_expiration_time: Duration,
    ) -> Result<Option<ResolvedQrCodes>> {
        loop {
            match qr_codes {
                QrCodes::Both { operator_data, user_qr_code, user_data, user_qr_code_string }
                    if operator_data.timestamp.elapsed() < operator_qr_expiration_time =>
                {
                    break Ok(Some(ResolvedQrCodes {
                        operator_data,
                        user_qr_code,
                        user_data,
                        user_qr_code_string,
                    }));
                }
                QrCodes::Operator { operator_data }
                    if operator_data.timestamp.elapsed() < operator_qr_expiration_time =>
                {
                    let Some((user_qr_code, user_data, user_qr_code_string)) =
                        self.scan_user_qr_code(orb, &operator_data).await?
                    else {
                        break Ok(None);
                    };
                    break Ok(Some(ResolvedQrCodes {
                        operator_data,
                        user_qr_code,
                        user_data,
                        user_qr_code_string,
                    }));
                }
                _ => {
                    let qr_capture_start = Instant::now();
                    let Some(operator_qr_code) =
                        self.scan_operator_qr_code(orb, Some(self.qr_scan_timeout)).await?
                    else {
                        break Ok(None);
                    };
                    if !check_signup_conditions(orb).await? {
                        continue;
                    }
                    let Some(operator_qr_code) =
                        self.handle_magic_operator_qr_code(orb, operator_qr_code).await?
                    else {
                        break Ok(None);
                    };
                    let Some((duration_since_shot_ms, operator_location_data)) = self
                        .verify_operator_qr_code(orb, &operator_qr_code, qr_capture_start)
                        .await?
                    else {
                        continue;
                    };
                    // a delay following the scan allows for a better user experience & increases the chance of
                    // not reusing any previous RGB frame for the next QR-code scan
                    if let Some(delay) =
                        QR_SCAN_INTERVAL.checked_sub(Duration::from_millis(duration_since_shot_ms))
                    {
                        sleep(delay).await;
                    }
                    let operator_data = OperatorData {
                        qr_code: operator_qr_code,
                        location_data: operator_location_data,
                        timestamp: Instant::now(),
                    };
                    let Some((user_qr_code, user_data, user_qr_code_string)) =
                        self.scan_user_qr_code(orb, &operator_data).await?
                    else {
                        break Ok(None);
                    };
                    break Ok(Some(ResolvedQrCodes {
                        operator_data,
                        user_qr_code,
                        user_data,
                        user_qr_code_string,
                    }));
                }
            }
        }
    }

    /// Scans the operator QR-code.
    /// Returns the operator data and the duration of the HTTP request
    /// used to check the operator ID for consistent UX.
    /// An artificial delay is added before returning for better UX.
    async fn scan_operator_qr_code(
        &self,
        orb: &mut Orb,
        timeout: Option<Duration>,
    ) -> Result<Option<qr_scan::operator::Data>> {
        orb.set_phase("Operator QR-code scanning").await;
        let qr_capture_start = Instant::now();
        loop {
            dd_incr!("main.count.signup.during.general.distributor_identification_request");

            let remaining_timeout = timeout
                .map(|timeout| {
                    timeout
                        .checked_sub(qr_capture_start.elapsed())
                        .ok_or(qr_scan::ScanError::Timeout)
                })
                .transpose();
            #[cfg_attr(not(feature = "internal-data-acquisition"), allow(unused_mut))]
            let mut result = match remaining_timeout {
                Ok(timeout) => {
                    if let Some(qr) = &self.operator_qr_code_override {
                        tracing::info!("Operator QR-code provided from CLI");
                        Ok(qr.clone())
                    } else {
                        qr_scan::Plan::<qr_scan::operator::Data>::new(timeout, false)
                            .run(orb)
                            .await?
                            .map(|(qr_code, _)| qr_code)
                    }
                }
                Err(err) => Err(err),
            };
            #[cfg(feature = "internal-data-acquisition")]
            if !self.data_acquisition {
                result = result.and_then(|data| {
                    if let qr_scan::operator::Data::Normal(data) = &data {
                        if data.signup_extension {
                            return Err(qr_scan::ScanError::Invalid);
                        }
                    }
                    Ok(data)
                });
            }
            orb.reset_rgb_camera().await?;
            match result {
                Ok(qr_code) => {
                    orb.ui.qr_scan_completed(QrScanSchema::Operator);
                    dd_incr!("main.count.global.distr_code_detected");
                    return Ok(Some(qr_code));
                }
                Err(qr_scan::ScanError::Invalid) => {
                    orb.ui.qr_scan_unexpected(
                        QrScanSchema::Operator,
                        QrScanUnexpectedReason::WrongFormat,
                    );
                    dd_incr!("main.count.signup.result.failure.distr_qr_code", "type:wrong_format");
                    continue; // retry
                }
                Err(qr_scan::ScanError::Timeout) => {
                    orb.ui.qr_scan_timeout(QrScanSchema::Operator);
                    dd_incr!("main.count.signup.result.failure.distr_qr_code", "type:timeout");
                    tracing::error!("Timeout while scanning operator QR-code");
                    return Ok(None);
                }
            }
        }
    }

    /// Scans the user QR-code.
    async fn scan_user_qr_code(
        &self,
        orb: &mut Orb,
        operator_data: &OperatorData,
    ) -> Result<Option<(qr_scan::user::Data, backend::user_status::UserData, String)>> {
        orb.set_phase("User QR-code scanning").await;
        dd_incr!("main.count.signup.during.general.user_identification_request");

        // QR capture starts now and timeout is updated after each scan attempt
        let qr_capture_start = Instant::now();
        loop {
            let scan_result = if let Some(new_timeout_ms) =
                self.qr_scan_timeout.checked_sub(qr_capture_start.elapsed())
            {
                if let Some(qr) = &self.user_qr_code_override {
                    tracing::info!("User QR-code provided from CLI");
                    Ok(qr.clone())
                } else {
                    orb.reset_rgb_camera().await?;
                    qr_scan::Plan::<qr_scan::user::Data>::new(Some(new_timeout_ms), false)
                        .run(orb)
                        .await?
                }
            } else {
                Err(qr_scan::ScanError::Timeout)
            };
            if let Some(result) = self
                .handle_user_qr_code(scan_result, orb, operator_data, Some(qr_capture_start))
                .await?
            {
                break Ok(result);
            }
        }
    }

    async fn handle_magic_operator_qr_code(
        &self,
        orb: &mut Orb,
        qr_code: qr_scan::operator::Data,
    ) -> Result<Option<qr_scan::user::Data>> {
        match qr_code {
            qr_scan::operator::Data::Normal(qr_code) => {
                tracing::info!("Operator QR-code detected: {qr_code:?}");
                Ok(Some(qr_code))
            }
            qr_scan::operator::Data::MagicResetMirror => {
                tracing::info!("Magic QR-code detected: Reset Mirror");
                dd_incr!("main.count.signup.during.general.magic_qr.reset_mirror");
                let result = self.reset_mirror_calibration(orb).await;
                if let Err(err) = &result {
                    tracing::error!("Failed to reset mirror calibration: {err}");
                }
                orb.ui.magic_qr_action_completed(result.is_ok());
                Ok(None)
            }
            qr_scan::operator::Data::MagicResetWifi => {
                tracing::info!("Magic QR-code detected: Reset Wi-Fi");
                dd_incr!("main.count.signup.during.general.magic_qr.reset_wifi");
                let result = self.reset_wifi_and_ensure_network(orb).await;
                if let Err(err) = &result {
                    tracing::error!("Failed to reset wifi: {err}");
                }
                orb.ui.magic_qr_action_completed(result.is_ok());
                Ok(None)
            }
        }
    }

    #[cfg_attr(not(feature = "internal-data-acquisition"), allow(unused_mut))]
    async fn handle_user_qr_code(
        &self,
        mut scan_result: Result<(qr_scan::user::Data, String), qr_scan::ScanError>,
        orb: &mut Orb,
        operator_data: &OperatorData,
        qr_capture_start: Option<Instant>,
    ) -> Result<Option<Option<(qr_scan::user::Data, backend::user_status::UserData, String)>>> {
        #[cfg(feature = "internal-data-acquisition")]
        if !self.data_acquisition {
            scan_result = scan_result.and_then(|(data, string)| {
                if data.signup_extension {
                    Err(qr_scan::ScanError::Invalid)
                } else {
                    Ok((data, string))
                }
            });
        }
        let (user_qr_code, user_qr_code_string) = match scan_result {
            Ok((user_qr_code, user_qr_code_string)) => {
                dd_incr!("main.count.signup.during.general.user_qr_code_detected");
                tracing::info!("User QR-code detected: {user_qr_code:?}");
                orb.ui.qr_scan_completed(QrScanSchema::User);

                // Filter out the operator QR code
                if user_qr_code.user_id == operator_data.qr_code.user_id {
                    orb.ui.qr_scan_unexpected(QrScanSchema::User, QrScanUnexpectedReason::Invalid);
                    tracing::info!("User QR-code is the same as the operator QR-code, retrying");
                    // Give time to remove the QR code from the front of the camera
                    sleep(Duration::from_millis(1500)).await;
                    #[cfg(not(feature = "integration_testing"))]
                    return Ok(None);
                }
                (user_qr_code, user_qr_code_string)
            }
            Err(qr_scan::ScanError::Invalid) => {
                orb.ui.qr_scan_unexpected(QrScanSchema::User, QrScanUnexpectedReason::WrongFormat);
                dd_incr!("main.count.signup.result.failure.user_qr_code", "type:wrong_format");
                tracing::error!("Invalid user QR-code format");
                return Ok(None);
            }
            Err(qr_scan::ScanError::Timeout) => {
                orb.ui.qr_scan_timeout(QrScanSchema::User);
                dd_incr!("main.count.signup.result.failure.user_qr_code", "type:timeout");
                tracing::error!("Timeout while scanning user QR-code");
                return Ok(Some(None));
            }
        };

        if operator_data.qr_code.signup_extension() || user_qr_code.signup_extension() {
            if user_qr_code.signup_extension() && operator_data.qr_code.signup_extension() {
                if let Some(SignupExtensionConfig { mode, parameters: _ }) = user_qr_code
                    .signup_extension_config
                    .as_ref()
                    .or(operator_data.qr_code.signup_extension_config.as_ref())
                {
                    dd_incr!("main.count.data_acquisition.mode", &format!("mode:{mode:?}"));
                    return Ok(Some(Some((
                        user_qr_code,
                        backend::user_status::UserData::default(),
                        user_qr_code_string,
                    ))));
                }
            }
            orb.ui.qr_scan_unexpected(QrScanSchema::User, QrScanUnexpectedReason::Invalid);
            dd_incr!("main.count.data_acquisition.failure.user_qr_code", "type:invalid_qr");
            tracing::error!(
                "Invalid user QR-code format for data acquisition. User QR-code: \
                 {user_qr_code:?}. Operator QR-code: {:?}",
                operator_data.qr_code
            );
            return Ok(None);
        }

        if let Some(user_data) =
            self.verify_user_qr_code(orb, &user_qr_code, operator_data, qr_capture_start).await?
        {
            return Ok(Some(Some((user_qr_code, user_data, user_qr_code_string))));
        }
        Ok(None)
    }

    /// Detects the user face.
    async fn detect_face(&self, orb: &mut Orb) -> Result<bool> {
        orb.set_phase("Face detection").await;
        let t = Instant::now();
        let Config { self_serve, .. } = *orb.config.lock().await;
        let face_detected = detect_face::Plan::new(if self_serve {
            DETECT_FACE_TIMEOUT_SELF_SERVE
        } else {
            DETECT_FACE_TIMEOUT
        })
        .run(orb)
        .await?;
        dd_timing!("main.time.signup.face_detection", t);
        if face_detected {
            tracing::info!("Face detected");
            dd_incr!("main.count.signup.during.general.face_detected");
        } else {
            tracing::info!("Face not detected");
            dd_incr!("main.count.signup.result.failure.face_detection", "type:timeout");
            notify_failed_signup(orb, Some(SignupFailReason::FaceNotFound));
        }
        Ok(face_detected)
    }

    async fn after_biometric_capture(
        &self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        capture_succeeded: bool,
        self_serve: bool,
    ) -> Result<()> {
        if self_serve {
            tracing::info!("Self-serve: Informing backend that biometric_capture has ended");
            orb.orb_relay
                .as_mut()
                .expect("orb_relay to exist")
                .send(self_serve::orb::v1::CaptureEnded {
                    success: capture_succeeded,
                    failure_feedback: debug_report.failure_feedback_capture_proto(),
                })
                .await
                .inspect_err(|e| tracing::error!("Relay: Failed to CaptureEnded: {e}"))?;
        }
        Ok(())
    }

    /// Performs the biometric capture.
    #[allow(clippy::too_many_lines)]
    #[cfg_attr(not(feature = "internal-data-acquisition"), allow(clippy::diverging_sub_expression))]
    async fn biometric_capture(
        &self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
    ) -> Result<Option<biometric_capture::Capture>> {
        #[cfg(feature = "allow-plan-mods")]
        if let Some(biometric_input) = self.biometric_input.as_deref() {
            return Ok(Some(load_biometric_input(biometric_input, debug_report).await?));
        }

        if !proceed_with_biometric_capture(orb).await? {
            return Ok(None);
        }

        if !self.detect_face(orb).await? {
            return Ok(None);
        }

        tracing::info!("Starting image notary");
        orb.start_image_notary(debug_report.signup_id.clone()).await?;

        orb.set_phase("Biometric capture").await;
        let t = Instant::now();
        let mut wavelengths = vec![(DEFAULT_IR_LED_WAVELENGTH, DEFAULT_IR_LED_DURATION)];
        wavelengths.extend_from_slice(EXTRA_IR_LED_WAVELENGTHS);
        let Config { self_serve, self_serve_biometric_capture_timeout, .. } =
            *orb.config.lock().await;
        let plan = biometric_capture::Plan::new(
            &wavelengths,
            Some(if self_serve {
                self_serve_biometric_capture_timeout
            } else {
                BIOMETRIC_CAPTURE_TIMEOUT
            }),
            debug_report.signup_extension_config.clone(),
            &orb.config.lock().await.clone(),
        );
        let biometric_capture::Output {
            capture,
            log,
            extension_report,
            capture_failure_feedback_messages,
        } = if let Some(SignupExtensionConfig { mode, parameters: _ }) =
            &debug_report.signup_extension_config
        {
            match mode {
                qr_scan::user::SignupMode::PupilContractionExtension => {
                    tracing::info!("Pupil Contraction extension: activated");
                    biometric_capture::pupil_contraction::Plan::from(plan).run(orb).await?
                }
                qr_scan::user::SignupMode::FocusSweepExtension => {
                    tracing::info!("Focus Sweep extension: activated");
                    biometric_capture::focus_sweep::Plan::from(plan).run(orb).await?
                }
                qr_scan::user::SignupMode::MirrorSweepExtension => {
                    tracing::info!("Mirror Sweep extension: activated");
                    biometric_capture::mirror_sweep::Plan::from(plan).run(orb).await?
                }
                qr_scan::user::SignupMode::MultiWavelength => {
                    tracing::info!("Multi-wavelength extension: activated");
                    biometric_capture::multi_wavelength::Plan::from(plan).run(orb).await?
                }
                qr_scan::user::SignupMode::Overcapture => {
                    tracing::info!("Overcapture extension: activated");
                    biometric_capture::overcapture::Plan::from(plan).run(orb).await?
                }
                qr_scan::user::SignupMode::Basic => plan.run(orb).await?,
            }
        } else {
            plan.run(orb).await?
        };
        dd_timing!("main.time.signup.biometric_capture", t);
        tracing::info!("Stopping image notary");

        if let Some(ref capture) = capture {
            let identification_image_ids = 'identification_image_ids: {
                #[cfg(feature = "internal-data-acquisition")]
                if self.data_acquisition {
                    break 'identification_image_ids ({
                        debug_report.biometric_capture_gps_location(
                            capture.latitude.unwrap_or(0.0),
                            capture.longitude.unwrap_or(0.0),
                        );

                        orb.save_identification_images(
                            capture.eye_left.clone(),
                            capture.eye_right.clone(),
                            capture.face_self_custody_candidate.clone(),
                        )
                        .await?
                    });
                }
                // From KK and onwards, we shall never have image IDs from the image notary as all signups are opt-out.
                // But we still need the image IDs for the personal custody package. So if the biometric capture
                // succeeds, we shall always get the images IDs.
                break 'identification_image_ids (IdentificationImages {
                    left_ir: capture.eye_left.ir_frame.image_id(&debug_report.signup_id),
                    right_ir: capture.eye_right.ir_frame.image_id(&debug_report.signup_id),
                    left_rgb: capture.eye_left.rgb_frame.image_id(&debug_report.signup_id),
                    right_rgb: capture.eye_right.rgb_frame.image_id(&debug_report.signup_id),
                    self_custody_candidate: capture
                        .face_self_custody_candidate
                        .rgb_frame
                        .image_id(&debug_report.signup_id),
                    ..Default::default()
                });
            };

            debug_report.insert_identification_images(identification_image_ids);
        }

        #[cfg(feature = "integration_testing")]
        let capture = {
            let mut capture = capture;
            if let Some(ci_hacks) = &self.ci_hacks {
                tracing::info!("CI hacks enabled");
                ci_hacks.replace_captured_eyes(&mut capture)?;
            }
            capture
        };

        let image_notary_log = orb.stop_image_notary().await?;

        debug_report.biometric_capture_feedback_messages(capture_failure_feedback_messages);
        debug_report.biometric_capture_history(log);
        debug_report.image_notary_history(image_notary_log);
        if let Some(report) = extension_report {
            debug_report.extension_report(report);
        }

        if let Some(capture) = capture {
            debug_report.rgb_net_metadata(
                capture.eye_left.rgb_net_estimate.clone(),
                capture.eye_right.rgb_net_estimate.clone(),
            );
            debug_report.biometric_capture_succeeded();
            orb.ui.biometric_capture_success();
            Ok(Some(capture))
        } else {
            tracing::error!("SIGNUP TIMEOUT");
            dd_incr!("main.count.signup.result.failure.biometric_capture", "type:timeout");
            notify_failed_signup(orb, Some(SignupFailReason::Timeout));
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
        orb.set_phase("Biometric pipeline").await;
        let pipeline = Box::pin(
            biometric_pipeline::Plan::new(capture, debug_report.signup_id.clone())?.run(orb),
        )
        .await;
        let pipeline = match pipeline {
            Ok(pipeline) => pipeline,
            Err(e) => {
                if let Some(e) = e.downcast_ref::<biometric_pipeline::Error>() {
                    match e {
                        biometric_pipeline::Error::Timeout => {
                            tracing::error!("Biometric pipeline failed: timeout");
                            dd_incr!(
                                "main.count.signup.result.failure.biometric_pipeline",
                                "type:timeout"
                            );
                        }
                        biometric_pipeline::Error::Agent => {
                            tracing::error!("Biometric pipeline failed: some agent failed");
                            dd_incr!(
                                "main.count.signup.result.failure.biometric_pipeline",
                                "type:agent"
                            );
                        }
                        biometric_pipeline::Error::Iris(error) => {
                            tracing::error!(
                                "Biometric pipeline failed due to iris agent: {}",
                                error
                            );
                            dd_incr!(
                                "main.count.signup.result.failure.biometric_pipeline",
                                "type:iris_agent"
                            );
                            debug_report.iris_model_error(Some(error.clone()));
                        }
                    }
                } else {
                    // In case we don't recognize the error,
                    // then it must come from a '?' in any
                    // biometric pipeline called method.
                    tracing::error!("Biometric pipeline failed: unknown error: {e:?}");
                    dd_incr!("main.count.signup.result.failure.biometric_pipeline", "type:unknown");
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
            pipeline.v2.eye_left.iris_normalized_image_resized.clone(),
            pipeline.v2.eye_right.iris_normalized_image_resized.clone(),
        );
        debug_report.mega_agent_one_config(pipeline.mega_agent_one_config.clone());
        debug_report.mega_agent_two_config(pipeline.mega_agent_two_config.clone());
        debug_report.face_identifier_results(pipeline.face_identifier_fraud_checks.clone());
        debug_report.self_custody_bundle(pipeline.face_identifier_bundle.clone().ok());
        debug_report.self_custody_thumbnail(pipeline.face_identifier_bundle.clone().ok());
        debug_report.occlusion_error(pipeline.occlusion.clone().err());
        tracing::info!("Occlusion Detection result: {:?}", pipeline.occlusion);
        log_iris_data(
            &pipeline.v2.eye_left.iris_code_shares,
            &pipeline.v2.eye_left.mask_code_shares,
            &pipeline.v2.eye_left.iris_code,
            &pipeline.v2.eye_left.mask_code,
            pipeline.v2.eye_left.iris_code_version.as_ref(),
            true,
            "master plan",
        );
        log_iris_data(
            &pipeline.v2.eye_right.iris_code_shares,
            &pipeline.v2.eye_right.mask_code_shares,
            &pipeline.v2.eye_right.iris_code,
            &pipeline.v2.eye_right.mask_code,
            pipeline.v2.eye_right.iris_code_version.as_ref(),
            false,
            "master plan",
        );
        #[cfg(feature = "allow-plan-mods")]
        if let Some(biometric_input) = &self.biometric_input {
            let codes = serde_json::to_string_pretty(&signup_post::format_pipeline(&pipeline))?;
            let json_path = biometric_input.join("codes.json");
            fs::write(&json_path, format!("{{\"codes\": {codes}\n}}")).await?;
            tracing::info!("Wrote iris codes to {}", json_path.display());
        }

        orb.ui.biometric_pipeline_success();
        Ok(Some(pipeline))
    }

    /// Performs the fraud checks.
    #[allow(clippy::too_many_lines)]
    async fn detect_fraud(
        &mut self,
        orb: &mut Orb,
        _debug_report: &mut debug_report::Builder,
        pipeline: Option<&biometric_pipeline::Pipeline>,
    ) -> Result<bool> {
        orb.set_phase("Fraud detection").await;
        let Some(_pipeline) = pipeline else {
            return Ok(false);
        };

        // FOSS: WE HAVE DELETED ALL FRAUD CHECKS

        Ok(false)
    }

    async fn enroll_user(
        &mut self,
        orb: &mut Orb,
        debug_report: &mut debug_report::Builder,
        capture: &biometric_capture::Capture,
        pipeline: Option<&biometric_pipeline::Pipeline>,
        signup_reason: SignupReason,
    ) -> enroll_user::Status {
        orb.set_phase("User enrollment").await;
        let t = Instant::now();
        let status = Box::pin(
            enroll_user::Plan {
                signup_id: debug_report.signup_id.clone(),
                operator_qr_code: debug_report.operator_qr_code.clone(),
                user_qr_code: debug_report.user_qr_code.clone(),
                s3_region_str: self.s3_region_str.clone(),
                capture,
                pipeline,
                signup_reason,
            }
            .run(orb),
        )
        .await;
        dd_timing!("main.time.signup.user_enrollment", t);

        debug_report.enrollment_status(status.clone());
        status
    }

    /// Uploads the signup data json and also trigger uploading of identification images on failure
    /// or fraud.
    async fn upload_debug_report(
        &self,
        orb: &mut Orb,
        debug_report: debug_report::Builder,
    ) -> Result<()> {
        let signup_id = debug_report.signup_id.clone();

        tracing::info!("After-signup phase - Uploading signup data");
        let t1 = Instant::now();
        let debug_report = debug_report.build(SystemTime::now(), orb.config.lock().await.clone());
        match upload_debug_report::request(&signup_id, &debug_report).await {
            Ok(()) => {
                dd_incr!("main.count.data_acquisition.upload.success.signup_json");
            }
            Err(e) => {
                dd_incr!("main.count.data_acquisition.upload.error.signup_json");
                tracing::error!("Uploading signup data failed: {e}");
            }
        }
        dd_timing!("main.time.signup.signup_json_upload", t1);

        Ok(())
    }

    async fn after_signup(&mut self, orb: &mut Orb, signup_result: SignupResult) -> Result<()> {
        let SignupResult { capture_start, debug_report, .. } = signup_result;
        if self.skip_pipeline() {
            // This is just to give the UI ring some time to reset.
            sleep(Duration::from_secs(5)).await;
            return Ok(());
        }
        let Some(debug_report) = debug_report else { return Ok(()) };

        tracing::info!("After-signup phase");
        dd_timing!("main.time.signup.full_signup", capture_start);

        let signup_status = debug_report.signup_status.clone();

        let enrollment_status = debug_report.enrollment_status.clone();
        let failure_feedback = debug_report.failure_feedback_after_capture_proto();
        Box::pin(self.upload_debug_report(orb, debug_report)).await?;

        if let Some(signup_status) = signup_status {
            Self::ui_complete_signup(orb, &signup_status, enrollment_status);
        }

        if orb.config.lock().await.self_serve {
            if let Some(relay) = orb.orb_relay.as_mut() {
                relay
                    .send(self_serve::orb::v1::SignupEnded {
                        success: signup_result.success,
                        failure_feedback,
                    })
                    .await
                    .inspect_err(|e| tracing::error!("Relay: Failed to SignupEnded: {e}"))?;
            }
        }

        Ok(())
    }

    fn ui_complete_signup(
        orb: &mut Orb,
        signup_status: &debug_report::SignupStatus,
        enrollment_status: Option<enroll_user::Status>,
    ) {
        match signup_status {
            SignupStatus::Success => orb.ui.signup_success(),
            SignupStatus::OrbFailure | SignupStatus::InternalError => {
                notify_failed_signup(orb, Some(SignupFailReason::Unknown));
            }
            SignupStatus::Fraud => notify_failed_signup(orb, Some(SignupFailReason::Verification)),
            SignupStatus::ServerFailure => {
                if let Some(enrollment_status) = enrollment_status {
                    match enrollment_status {
                        enroll_user::Status::Success => unreachable!(),
                        enroll_user::Status::SoftwareVersionUnknown
                        | enroll_user::Status::SoftwareVersionOutdated => {
                            notify_failed_signup(
                                orb,
                                Some(SignupFailReason::SoftwareVersionBlocked),
                            );
                        }
                        enroll_user::Status::SignupVerificationNotSuccessful => {
                            notify_failed_signup(orb, Some(SignupFailReason::Verification));
                        }
                        enroll_user::Status::SignatureCalculationError
                        | enroll_user::Status::Error
                        | enroll_user::Status::ServerError => {
                            notify_failed_signup(orb, Some(SignupFailReason::Server));
                        }
                    }
                } else {
                    tracing::error!("Server failure without enrollment status: This is a bug!");
                    notify_failed_signup(orb, Some(SignupFailReason::Unknown));
                }
            }
            // TODO: New UX?
            SignupStatus::OrbRelayFailure | SignupStatus::AppIncompatible => {
                notify_failed_signup(orb, Some(SignupFailReason::SoftwareVersionDeprecated));
            }
        }
    }

    /// Checks if `qr_code` is a valid operator QR-code through the backend.
    #[allow(clippy::cast_possible_truncation)]
    async fn verify_operator_qr_code(
        &self,
        orb: &mut Orb,
        qr_code: &qr_scan::user::Data,
        qr_capture_start: Instant,
    ) -> Result<Option<(u64, backend::operator_status::LocationData)>> {
        if qr_code.signup_extension() || self.operator_qr_code_override.is_some() {
            return Ok(Some((0, backend::operator_status::LocationData {
                team_operating_country: "DEV".to_string(),
                session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
                stationary_location_coordinates: None,
            })));
        }
        let http_start = Instant::now();
        match backend::operator_status::request(qr_code).await {
            Ok(backend::operator_status::Status { valid: true, location_data, reason: _ }) => {
                let location_data = location_data
                    .expect("to always have a result from the backend if valid == true");
                orb.ui.qr_scan_success(QrScanSchema::Operator);
                dd_incr!("main.count.global.distr_code_validated");
                tracing::info!("Operator QR-code validated: {qr_code:?}");
                dd_timing!("main.time.signup.distr_qr_code_capture", qr_capture_start);
                return Ok(Some((http_start.elapsed().as_millis() as u64, location_data)));
            }
            Ok(backend::operator_status::Status { valid: false, .. }) => {
                orb.ui.qr_scan_fail(QrScanSchema::Operator);
                dd_incr!("main.count.signup.result.failure.distr_qr_code", "type:invalid_qr");
            }
            Err(_) => {
                orb.ui.qr_scan_fail(QrScanSchema::Operator);
            }
        }
        Ok(None)
    }

    /// Checks if `qr_code` is a valid user QR-code through the backend.
    async fn verify_user_qr_code(
        &self,
        orb: &mut Orb,
        user_qr_code: &qr_scan::user::Data,
        operator_data: &OperatorData,
        qr_capture_start: Option<Instant>,
    ) -> Result<Option<backend::user_status::UserData>> {
        let Config {
            user_qr_validation_use_full_operator_qr,
            user_qr_validation_use_only_operator_location,
            ..
        } = *orb.config.lock().await;
        match backend::user_status::request(
            user_qr_code,
            operator_data,
            user_qr_validation_use_full_operator_qr,
            user_qr_validation_use_only_operator_location,
        )
        .await
        {
            Ok(Some(user_data)) => {
                orb.ui.qr_scan_success(QrScanSchema::User);
                dd_incr!("main.count.signup.during.general.user_qr_code_validate");
                tracing::info!("User QR-code validated: {user_qr_code:?}");
                if let Some(qr_capture_start) = qr_capture_start {
                    dd_timing!("main.time.signup.user_qr_code_capture", qr_capture_start);
                }
                return Ok(Some(user_data));
            }
            Ok(None) => {
                orb.ui.qr_scan_fail(QrScanSchema::User);
                dd_incr!("main.count.signup.result.failure.user_qr_code", "type:invalid_qr");
            }
            Err(_) => {
                orb.ui.qr_scan_fail(QrScanSchema::User);
                dd_incr!(
                    "main.count.signup.result.failure.user_qr_code",
                    "type:validation_network_error"
                );
            }
        }
        Ok(None)
    }

    /// Checks if Orb OS is in an allowed version range.
    async fn is_orb_os_version_allowed(&self, debug_report: &mut debug_report::Builder) -> bool {
        match backend::orb_os_status::request().await {
            Ok(orb_os_status::OrbOsVersionCheckResponse {
                status: OrbOsVersionStatus::Allowed,
                ..
            }) => return true,
            Ok(orb_os_status::OrbOsVersionCheckResponse { status, error }) => {
                dd_incr!("main.count.signup.result.failure.orb_os_version");
                tracing::error!("Orb OS version check failed. Status: {status:?} Error: {error:?}");
                debug_report.signup_server_failure();
                debug_report.enrollment_status(enroll_user::Status::SoftwareVersionOutdated);
            }
            Err(e) => tracing::error!("Orb OS version check request failed: {e:?}"),
        }
        false
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn build_pcp(
        &self,
        orb: &mut Orb,
        credentials: personal_custody_package::Credentials,
        capture: &biometric_capture::Capture,
        pipeline: Option<&biometric_pipeline::Pipeline>,
        debug_report: &debug_report::Builder,
        signup_reason: SignupReason,
    ) -> Result<Option<PersonalCustodyPackages>> {
        macro_rules! data_error {
            ($field:literal) => {
                data_error!(
                    concat!("Image self-custody upload failed due to missing `", $field, "``"),
                    concat!("type:missing_", $field)
                )
            };
            ($message:expr, $dd_type:expr) => {
                tracing::error!($message);
                dd_incr!("main.count.signup.result.failure.upload_custody_images", $dd_type);
                notify_failed_signup(orb, None);
                return Ok(None);
            };
        }

        let Some(face_identifier_bundle) =
            pipeline.as_ref().and_then(|p| p.face_identifier_bundle.as_ref().ok())
        else {
            data_error!("face_identifier_bundle");
        };
        if let Some(error) = &face_identifier_bundle.error {
            data_error!(
                "Face identifier bundle contains an error: {error:?}",
                "type:face_identifier_bundle_error"
            );
        }
        let Some(face_identifier_thumbnail) = &face_identifier_bundle.thumbnail else {
            data_error!("face_identifier_bundle.thumbnail");
        };
        let Some(face_identifier_thumbnail_image) = &face_identifier_thumbnail.image else {
            data_error!("face_identifier_bundle.thumbnail.image");
        };
        let Some(face_identifier_embeddings) = &face_identifier_bundle.embeddings else {
            data_error!("face_identifier_bundle.embeddings");
        };
        let Some(face_identifier_inference_backend) = &face_identifier_bundle.inference_backend
        else {
            data_error!("face_identifier_bundle.inference_backend");
        };
        let Some(left_normalized_iris_image) =
            pipeline.as_ref().and_then(|p| p.v2.eye_left.iris_normalized_image.as_ref())
        else {
            data_error!("v2.eye_left.iris_normalized_image");
        };
        let Some(right_normalized_iris_image) =
            pipeline.as_ref().and_then(|p| p.v2.eye_right.iris_normalized_image.as_ref())
        else {
            data_error!("v2.eye_right.iris_normalized_image");
        };
        let Some(left_normalized_iris_image_resized) =
            pipeline.as_ref().and_then(|p| p.v2.eye_left.iris_normalized_image_resized.as_ref())
        else {
            data_error!("v2.eye_left.iris_normalized_image_resized");
        };
        let Some(right_normalized_iris_image_resized) =
            pipeline.as_ref().and_then(|p| p.v2.eye_right.iris_normalized_image_resized.as_ref())
        else {
            data_error!("v2.eye_right.iris_normalized_image_resized");
        };

        let (left_normalized_iris_image, left_normalized_iris_mask) =
            left_normalized_iris_image.serialized_image_and_mask();
        let (right_normalized_iris_image, right_normalized_iris_mask) =
            right_normalized_iris_image.serialized_image_and_mask();
        let (left_normalized_iris_image_resized, left_normalized_iris_mask_resized) =
            left_normalized_iris_image_resized.serialized_image_and_mask();
        let (right_normalized_iris_image_resized, right_normalized_iris_mask_resized) =
            right_normalized_iris_image_resized.serialized_image_and_mask();
        let pipeline = personal_custody_package::Pipeline {
            face_identifier_thumbnail_image: face_identifier_thumbnail_image
                .as_ndarray()
                .to_owned(),
            face_identifier_embeddings: face_identifier_embeddings.clone(),
            face_identifier_inference_backend: face_identifier_inference_backend.clone(),
            left_normalized_iris_image,
            left_normalized_iris_mask,
            left_normalized_iris_image_resized,
            left_normalized_iris_mask_resized,
            right_normalized_iris_image,
            right_normalized_iris_mask,
            right_normalized_iris_image_resized,
            right_normalized_iris_mask_resized,
            left_iris_code_shares: pipeline
                .as_ref()
                .map(|p| p.v2.eye_left.iris_code_shares.clone()),
            left_iris_code: pipeline.as_ref().map(|p| p.v2.eye_left.iris_code.clone()),
            left_mask_code_shares: pipeline
                .as_ref()
                .map(|p| p.v2.eye_left.mask_code_shares.clone()),
            left_mask_code: pipeline.as_ref().map(|p| p.v2.eye_left.mask_code.clone()),
            right_iris_code_shares: pipeline
                .as_ref()
                .map(|p| p.v2.eye_right.iris_code_shares.clone()),
            right_iris_code: pipeline.as_ref().map(|p| p.v2.eye_right.iris_code.clone()),
            right_mask_code_shares: pipeline
                .as_ref()
                .map(|p| p.v2.eye_right.mask_code_shares.clone()),
            right_mask_code: pipeline.as_ref().map(|p| p.v2.eye_right.mask_code.clone()),
            iris_version: pipeline.as_ref().map(|p| p.v2.iris_version.clone()),
        };
        let capture_start = debug_report.start_timestamp;
        let signup_id = debug_report.signup_id.clone();
        let identification_image_ids = debug_report
            .identification_image_ids
            .clone()
            .expect("identification images to always exist");

        orb.ui.starting_enrollment();
        let packages = Box::pin(
            personal_custody_package::Plan {
                capture_start,
                signup_id,
                identification_image_ids,
                capture: capture.clone(),
                pipeline,
                credentials,
                signup_reason,
                location_data: debug_report.location_data.clone(),
            }
            .run(),
        )
        .await?;
        Ok(Some(packages))
    }

    async fn upload_pcp_tier_0(
        &self,
        orb: &mut Orb,
        signup_id: &SignupId,
        user_id: &str,
        data: Vec<u8>,
        checksum: Digest,
        tier: Option<u8>,
    ) -> Result<bool> {
        const RETRIES_COUNT: usize = 6;
        tracing::info!("Start uploading personal custody package");
        let t = Instant::now();
        for i in 0..RETRIES_COUNT {
            let response = backend::upload_personal_custody_package::request(
                signup_id,
                user_id,
                checksum.as_ref(),
                &data,
                tier,
                &orb.config,
            )
            .await;
            match response {
                Ok(()) => {
                    dd_timing!("main.time.signup.upload_custody_images", t);
                    tracing::info!(
                        "Personal custody package uploading completed in: {}ms",
                        t.elapsed().as_millis()
                    );
                    return Ok(true);
                }
                Err(err) => {
                    tracing::error!("UPLOAD PERSONAL CUSTODY PACKAGE ERROR: {err:?}");
                    dd_incr!(
                        "main.count.http.upload_custody_images.error.network_error",
                        "error_type:normal"
                    );
                    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
                        if let Some(status) = reqwest_err.status() {
                            if status.is_client_error() {
                                dd_incr!(
                                    "main.count.signup.result.failure.upload_custody_images",
                                    "type:network_error",
                                    "subtype:signup_request"
                                );
                                break;
                            }
                        }
                    }
                    if i == RETRIES_COUNT - 1 {
                        dd_incr!(
                            "main.count.signup.result.failure.upload_custody_images",
                            "type:network_error",
                            "subtype:signup_request"
                        );
                    }
                }
            }
        }
        notify_failed_signup(orb, Some(SignupFailReason::UploadCustodyImages));
        Ok(false)
    }

    /// Helper function to avoid writing cfg gates everywhere.
    #[allow(clippy::unused_self)]
    fn has_biometric_input(&self) -> bool {
        #[cfg(not(feature = "allow-plan-mods"))]
        return false;
        #[cfg(feature = "allow-plan-mods")]
        return self.biometric_input.is_some();
    }

    /// Helper function to avoid writing cfg gates everywhere.
    #[allow(clippy::unused_self)]
    fn skip_fraud_checks(&self) -> bool {
        #[cfg(not(feature = "allow-plan-mods"))]
        return false;
        #[cfg(feature = "allow-plan-mods")]
        return self.skip_fraud_checks;
    }

    /// Helper function to avoid writing cfg gates everywhere.
    #[allow(clippy::unused_self)]
    fn skip_pipeline(&self) -> bool {
        #[cfg(not(feature = "allow-plan-mods"))]
        return false;
        #[cfg(feature = "allow-plan-mods")]
        return self.skip_pipeline;
    }
}

impl QrCodes {
    fn with_user_qr_code(
        &self,
        user_qr_code: qr_scan::user::Data,
        user_data: backend::user_status::UserData,
        user_qr_code_string: String,
    ) -> Self {
        match self {
            QrCodes::Operator { operator_data } => QrCodes::Both {
                operator_data: operator_data.clone(),
                user_qr_code,
                user_data,
                user_qr_code_string,
            },
            QrCodes::Both { .. } => panic!("user QR code is already present"),
            QrCodes::None => panic!("no operator QR code"),
        }
    }

    fn operator_timestamp(&self) -> Option<Instant> {
        match self {
            QrCodes::Operator { operator_data } | QrCodes::Both { operator_data, .. } => {
                Some(operator_data.timestamp)
            }
            QrCodes::None => None,
        }
    }
}

impl TryInto<personal_custody_package::Credentials> for ResolvedQrCodes {
    type Error = ();

    fn try_into(self) -> Result<personal_custody_package::Credentials, Self::Error> {
        let ResolvedQrCodes { operator_data, user_data, user_qr_code, user_qr_code_string } = self;
        if let (
            Some(backend_iris_public_key),
            Some(backend_iris_encrypted_private_key),
            Some(backend_normalized_iris_public_key),
            Some(backend_normalized_iris_encrypted_private_key),
            Some(backend_face_public_key),
            Some(backend_face_encrypted_private_key),
            Some(self_custody_user_public_key),
        ) = (
            user_data.backend_iris_public_key,
            user_data.backend_iris_encrypted_private_key,
            user_data.backend_normalized_iris_public_key,
            user_data.backend_normalized_iris_encrypted_private_key,
            user_data.backend_face_public_key,
            user_data.backend_face_encrypted_private_key,
            user_data.self_custody_user_public_key,
        ) {
            Ok(personal_custody_package::Credentials {
                operator_qr_code: operator_data.qr_code,
                user_qr_code,
                user_qr_code_string,
                backend_iris_public_key,
                backend_iris_encrypted_private_key,
                backend_normalized_iris_public_key,
                backend_normalized_iris_encrypted_private_key,
                backend_face_public_key,
                backend_face_encrypted_private_key,
                backend_tier2_public_key: user_data.backend_tier2_public_key,
                backend_tier2_encrypted_private_key: user_data.backend_tier2_encrypted_private_key,
                self_custody_user_public_key,
                pcp_version: user_data.pcp_version,
            })
        } else {
            Err(())
        }
    }
}

/// Notify to operator & user that signup failed with LED, sound and optionally a voice
pub fn notify_failed_signup(orb: &mut Orb, reason: Option<SignupFailReason>) {
    orb.ui.signup_fail(reason.unwrap_or(SignupFailReason::Unknown));
}

/// Loads biometric data from a directory instead of running biometric capture.
#[cfg(feature = "allow-plan-mods")]
async fn load_biometric_input(
    biometric_input: &Path,
    debug_report: &mut debug_report::Builder,
) -> Result<biometric_capture::Capture> {
    let ir_left_path = first_file_in_dir(&biometric_input.join("identification/ir/left"))?;
    let ir_right_path = first_file_in_dir(&biometric_input.join("identification/ir/right"))?;
    let rgb_left_path = first_file_in_dir(&biometric_input.join("identification/rgb/left"))?;
    let rgb_right_path = first_file_in_dir(&biometric_input.join("identification/rgb/right"))?;
    let rgb_self_custody_candidate_path =
        first_file_in_dir(&biometric_input.join("identification/rgb/self_custody_candidate"))?;

    let ir_left =
        spawn_blocking(|| camera::ir::Frame::read_png(File::open(ir_left_path)?)).await??;
    let ir_right =
        spawn_blocking(|| camera::ir::Frame::read_png(File::open(ir_right_path)?)).await??;
    let rgb_left =
        spawn_blocking(|| camera::rgb::Frame::read_png(File::open(rgb_left_path)?)).await??;
    let rgb_right =
        spawn_blocking(|| camera::rgb::Frame::read_png(File::open(rgb_right_path)?)).await??;
    let rgb_self_custody_candidate = spawn_blocking(|| {
        camera::rgb::Frame::read_png(File::open(rgb_self_custody_candidate_path)?)
    })
    .await??;

    let ir_left2 = ir_left.clone();
    let ir_right2 = ir_right.clone();
    let rgb_left2 = rgb_left.clone();
    let rgb_right2 = rgb_right.clone();
    let rgb_self_custody_candidate2 = rgb_self_custody_candidate.clone();

    // We use a single Python context as it will fail with: "LogicError: explicit_context_dependent failed: invalid
    // device context - no currently active context?"
    let (
        ir_net_estimate_left,
        ir_net_estimate_right,
        rgb_net_estimate_left,
        rgb_net_estimate_right,
        rgb_net_estimate_self_custody_candidate,
    ): (
        ir_net::EstimateOutput,
        ir_net::EstimateOutput,
        rgb_net::EstimateOutput,
        rgb_net::EstimateOutput,
        rgb_net::EstimateOutput,
    ) = spawn_blocking(move || {
        Python::with_gil(|py| {
            init_sys_argv(py);
            Ok((
                python::ir_net::estimate_once(py, &ir_left2)?,
                python::ir_net::estimate_once(py, &ir_right2)?,
                python::rgb_net::estimate_once(py, &rgb_left2)?,
                python::rgb_net::estimate_once(py, &rgb_right2)?,
                python::rgb_net::estimate_once(py, &rgb_self_custody_candidate2)?,
            ))
        }) as Result<_>
    })
    .await??;

    debug_report.biometric_capture_succeeded();
    Ok(biometric_capture::Capture {
        eye_left: biometric_capture::EyeCapture {
            ir_frame: ir_left,
            ir_frame_940nm: None,
            ir_frame_740nm: None,
            ir_net_estimate: ir_net_estimate_left,
            rgb_frame: rgb_left,
            rgb_net_estimate: rgb_net_estimate_left,
        },
        eye_right: biometric_capture::EyeCapture {
            ir_frame: ir_right,
            ir_frame_940nm: None,
            ir_frame_740nm: None,
            ir_net_estimate: ir_net_estimate_right,
            rgb_frame: rgb_right,
            rgb_net_estimate: rgb_net_estimate_right,
        },
        face_self_custody_candidate: SelfCustodyCandidate {
            rgb_frame: rgb_self_custody_candidate,
            rgb_net_eye_landmarks: rgb_net_estimate_self_custody_candidate
                .primary()
                .map(|prediction| (prediction.landmarks.left_eye, prediction.landmarks.right_eye))
                .ok_or_else(|| eyre!("rgb_net prediction is missing"))?,
            rgb_net_bbox: rgb_net_estimate_self_custody_candidate
                .primary()
                .ok_or_else(|| eyre!("rgb_net prediction is missing"))?
                .bbox
                .coordinates,
        },
        ..Default::default()
    })
}

#[cfg_attr(not(feature = "allow-plan-mods"), expect(dead_code))]
fn first_file_in_dir(dir: &Path) -> Result<PathBuf> {
    Ok(WalkDir::new(dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file())
        .ok_or_else(|| eyre!("{} is empty", dir.display()))?
        .into_path())
}

async fn check_signup_conditions(orb: &mut Orb) -> Result<bool> {
    if let Some(report) = orb.net_monitor.last_report()? {
        // Drop the mutex lock fast.
        let Config { block_signup_when_no_internet, .. } = *orb.config.lock().await;
        if block_signup_when_no_internet && report.is_no_internet() {
            orb.ui.no_internet_for_signup();
            dd_incr!("main.count.signup.result.failure.internet_check", "type:too_slow_to_start");
            return Ok(false);
        }
        if report.is_slow_internet() {
            orb.ui.slow_internet_for_signup();
            dd_incr!("main.count.signup.result.failure.internet_check", "type:too_slow_to_start");
            return Ok(true);
        }
    }
    Ok(true)
}

async fn proceed_with_biometric_capture(orb: &mut Orb) -> Result<bool> {
    let Config {
        self_serve,
        self_serve_app_skip_capture_trigger,
        self_serve_app_capture_trigger_timeout,
        ..
    } = *orb.config.lock().await;
    if !self_serve || self_serve_app_skip_capture_trigger {
        // Biometric capture not gated by a user action. Continue.
        orb.ui.signup_start();
        return Ok(true);
    }

    let orb_relay = orb.orb_relay.as_mut().expect("orb_relay to exist");

    tracing::info!("Waiting for self-serve biometric-capture trigger...");
    if let Err(e) = orb_relay
        .wait_for_msg::<self_serve::app::v1::StartCapture>(self_serve_app_capture_trigger_timeout)
        .await
    {
        if let Err(e) = orb_relay.send(self_serve::orb::v1::CaptureTriggerTimeout {}).await {
            tracing::warn!("failed to send CaptureTriggerTimeout: {e}");
        };
        orb.ui.signup_fail(SignupFailReason::Timeout);
        tracing::warn!("Self-serve biometric-capture start was not triggered: {e}");
        return Ok(false);
    };

    tracing::info!("Self-serve biometric-capture start triggered");
    orb.ui.signup_start();

    tracing::info!("Self-serve: Informing orb-relay that biometric_capture has started");
    orb_relay
        .send(self_serve::orb::v1::CaptureStarted {})
        .await
        .inspect_err(|e| tracing::error!("Relay: Failed to CaptureStarted: {e}"))?;

    Ok(true)
}

async fn orb_relay_announce_orb_id(
    orb: &mut Orb,
    orb_relay_app_id: String,
    is_self_serve_enabled: bool,
    reties: u32,
    timeout: Duration,
    wait_for_pending_messages: Duration,
    wait_for_shutdown: Duration,
) -> Result<()> {
    let mut relay = Client::new_as_orb(
        RELAY_BACKEND_URL.to_string(),
        get_orb_token()?,
        ORB_ID.to_string(),
        orb_relay_app_id,
    );
    if let Err(e) = relay.connect().await {
        dd_incr!("main.count.orb_relay.failure.connect");
        return Err(eyre::eyre!("Relay: Failed to connect: {e}"));
    }
    for _ in 0..reties {
        let now = Instant::now();
        if let Ok(()) = relay
            .send_blocking(
                common::v1::AnnounceOrbId {
                    orb_id: ORB_ID.to_string(),
                    mode_type: if is_self_serve_enabled {
                        common::v1::announce_orb_id::ModeType::SelfServe.into()
                    } else {
                        common::v1::announce_orb_id::ModeType::Legacy.into()
                    },
                    hardware_type: if identification::HARDWARE_VERSION.contains("Diamond") {
                        common::v1::announce_orb_id::HardwareType::Diamond.into()
                    } else {
                        common::v1::announce_orb_id::HardwareType::Pearl.into()
                    },
                },
                timeout,
            )
            .await
        {
            // Happy path. We have successfully announced and acknowledged the OrbId.
            dd_timing!("main.time.orb_relay.announce_orb_id", now);
            orb.orb_relay = if is_self_serve_enabled {
                Some(relay)
            } else {
                relay.graceful_shutdown(wait_for_pending_messages, wait_for_shutdown).await;
                None
            };
            return Ok(());
        }
        dd_incr!("main.count.orb_relay.retry.send.announce_orb_id");
        tracing::error!("Relay: Failed to AnnounceOrbId. Retrying...");
        relay.reconnect().await?;
        if relay.has_pending_messages().await? > 0 {
            sleep(Duration::from_secs(1)).await;
        }
    }
    dd_incr!("main.count.orb_relay.failure.send.announce_orb_id");
    Err(eyre::eyre!("Relay: Failed to send AnnounceOrbId after a reconnect"))
}

#[cfg(all(test, feature = "internal-data-acquisition"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_operator_user_predefined_biometric_collection_qr_codes() -> Result<()> {
        let mut fake_orb = Orb::builder().build().await?;
        let ms_base = MasterPlan::builder()
            .qr_scan_timeout(Duration::from_millis(10))
            .s3(orb_wld_data_id::S3Region::EuWest1, "eu-west-1".to_owned());

        // Operator QR code vanilla + data acquisition User QR code: should fail as no data acquisition mode is specified.
        {
            let ms = ms_base
                .clone()
                .operator_qr_code(Some(Some("userid:d6dea23a-32ea-420d-baaa-a94d6a7702de:1")))?
                .user_qr_code(Some(Some("userid:8f8a72dc-2543-451c-b40d-20429ea02abc:1::")))?
                .build()
                .await?;
            let op_code =
                ms.scan_operator_qr_code(&mut fake_orb, None).await?.expect("op_code exists");
            let op_code = ms
                .handle_magic_operator_qr_code(&mut fake_orb, op_code)
                .await?
                .expect("op_code not magic");
            let operator_data = OperatorData {
                qr_code: op_code,
                location_data: backend::operator_status::LocationData::default(),
                timestamp: Instant::now(),
            };
            let user_code = ms.scan_user_qr_code(&mut fake_orb, &operator_data).await?;
            assert!(user_code.is_none());
        }

        // Operator QR code vanilla + data acquisition User QR code: should fail as Operator is vanilla.
        {
            let ms = ms_base
                .clone()
                .operator_qr_code(Some(Some("userid:d6dea23a-32ea-420d-baaa-a94d6a7702de:1")))?
                .user_qr_code(Some(Some("userid:8f8a72dc-2543-451c-b40d-20429ea02abc:1::4::")))?
                .build()
                .await?;
            let op_code =
                ms.scan_operator_qr_code(&mut fake_orb, None).await?.expect("op_code exists");
            let op_code = ms
                .handle_magic_operator_qr_code(&mut fake_orb, op_code)
                .await?
                .expect("op_code not magic");
            let operator_data = OperatorData {
                qr_code: op_code,
                location_data: backend::operator_status::LocationData::default(),
                timestamp: Instant::now(),
            };
            let user_code = ms.scan_user_qr_code(&mut fake_orb, &operator_data).await?;
            assert!(user_code.is_none());
        }

        // Operator QR code data acquisition + vanilla User QR code: should fail as User is vanilla.
        {
            let ms = ms_base
                .clone()
                .operator_qr_code(Some(Some("userid:d6dea23a-32ea-420d-baaa-a94d6a7702de:1::4::")))?
                .user_qr_code(Some(Some("userid:8f8a72dc-2543-451c-b40d-20429ea02abc:1")))?
                .build()
                .await?;
            let op_code =
                ms.scan_operator_qr_code(&mut fake_orb, None).await?.expect("op_code exists");
            let op_code = ms
                .handle_magic_operator_qr_code(&mut fake_orb, op_code)
                .await?
                .expect("op_code not magic");
            let operator_data = OperatorData {
                qr_code: op_code,
                location_data: backend::operator_status::LocationData::default(),
                timestamp: Instant::now(),
            };
            let user_code = ms.scan_user_qr_code(&mut fake_orb, &operator_data).await?;
            assert!(user_code.is_none());
        }

        // Operator QR code data acquisition + data acquisition User QR code: should pass.
        {
            let ms = ms_base
                .clone()
                .operator_qr_code(Some(Some("userid:d6dea23a-32ea-420d-baaa-a94d6a7702de:1::4::")))?
                .user_qr_code(Some(Some("userid:8f8a72dc-2543-451c-b40d-20429ea02abc:1::")))?
                .build()
                .await?;
            let op_code =
                ms.scan_operator_qr_code(&mut fake_orb, None).await?.expect("op_code exists");
            let op_code = ms
                .handle_magic_operator_qr_code(&mut fake_orb, op_code)
                .await?
                .expect("op_code not magic");
            let operator_data = OperatorData {
                qr_code: op_code,
                location_data: backend::operator_status::LocationData::default(),
                timestamp: Instant::now(),
            };
            let user_code = ms.scan_user_qr_code(&mut fake_orb, &operator_data).await?;
            assert!(user_code.is_some());
        }

        // Operator QR code data acquisition + data acquisition User QR code: should fails during parsing as User is
        // opt-out.
        {
            let ms = ms_base
                .clone()
                .operator_qr_code(Some(Some("userid:d6dea23a-32ea-420d-baaa-a94d6a7702de:1::4::")))?
                .user_qr_code(Some(Some("userid:8f8a72dc-2543-451c-b40d-20429ea02abc:0::")));
            assert!(ms.is_err());
        }

        Ok(())
    }
}
