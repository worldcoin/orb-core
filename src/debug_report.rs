//! Debug report structures.

#![allow(missing_docs)]
#![allow(clippy::default_trait_access)]

use crate::{
    agents::{
        camera, image_notary,
        python::{
            face_identifier::{
                self,
                types::{BBox, Thumbnail as FIThumbnail},
                Bundle as FIBundle,
            },
            ir_net,
            iris::{self, NormalizedIris},
            mega_agent_one, mega_agent_two, rgb_net,
        },
    },
    backend::{
        self,
        endpoints::BACKEND,
        operator_status::{self, Coordinates},
    },
    config::Config,
    consts::{
        AUTOFOCUS_MAX, AUTOFOCUS_MIN, BIOMETRIC_CAPTURE_TIMEOUT, BUTTON_LONG_PRESS_DURATION,
        CONFIG_UPDATE_INTERVAL, CONTINUOUS_CALIBRATION_REDUCER, DEFAULT_IR_LED_DURATION,
        DEFAULT_IR_LED_WAVELENGTH, DETECT_FACE_TIMEOUT, DETECT_FACE_TIMEOUT_SELF_SERVE,
        EXTRA_IR_LED_WAVELENGTHS, IRIS_SCORE_MIN, IRIS_SHARPNESS_MIN, IR_CAMERA_DEFAULT_EXPOSURE,
        IR_CAMERA_DEFAULT_GAIN, IR_CAMERA_FRAME_RATE, IR_EYE_SAVE_FPS, IR_FACE_SAVE_FPS,
        IR_FOCUS_DISTANCE, IR_FOCUS_RANGE, IR_FOCUS_RANGE_SMALL, IR_LED_MAX_DURATION,
        IR_VOICE_TIME_INTERVAL, NUM_SHARP_IR_FRAMES, QR_SCAN_TIMEOUT, RGB_DEFAULT_HEIGHT,
        RGB_DEFAULT_WIDTH, RGB_EXPOSURE_RANGE, RGB_FPS, RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH,
        RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH, RGB_SAVE_FPS, SOUND_CARD_NAME, THERMAL_HEIGHT,
        THERMAL_SAVE_FPS, THERMAL_WIDTH, USER_LED_DEFAULT_BRIGHTNESS,
    },
    identification::{GIT_VERSION, ORB_ID, ORB_OS_VERSION},
    mcu::main::IrLed,
    plans::{
        self,
        biometric_capture::{self, CaptureFailureFeedbackMessage, ExtensionReport},
        enroll_user,
        fraud_check::{self, PipelineFailureFeedbackMessage},
        qr_scan::{self, user::SignupExtensionConfig},
    },
    time_series::TimeSeries,
    timestamped::Timestamped,
    utils::{ip_geo_info, serializable_instant::SerializableInstant, RkyvNdarray},
};
use ai_interface::PyError;
use derivative::Derivative;
use eyre::Result;
#[cfg(test)]
use mock_instant::Instant;
use ndarray::prelude::*;
use orb_relay_messages::self_serve;
use orb_wld_data_id::{ImageId, SignupId};
use schemars::JsonSchema;
use serde::Serialize;
#[cfg(not(test))]
use std::time::Instant;
use std::{
    default::Default,
    ops::RangeInclusive,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DEBUG_REPORT_VERSION: &str =
    "73a9723b689d1688abb42c7c42de0568bf1d459755e309a5e149d728f70500a5";

#[derive(Clone, Serialize, JsonSchema)]
pub struct Bundle {
    pub error: Option<PyError>,

    pub thumbnail: Option<Thumbnail>,
    pub embeddings: Option<Vec<Embedding>>,
    pub inference_backend: Option<String>,
}

impl From<FIBundle> for Bundle {
    fn from(bundle: FIBundle) -> Self {
        Bundle {
            error: bundle.error,
            thumbnail: bundle.thumbnail.map(
                |FIThumbnail {
                     border,
                     bounding_box,
                     image: _,
                     rotated_angle,
                     shape,
                     original_shape,
                     original_image,
                 }| {
                    Thumbnail {
                        border,
                        bounding_box,
                        rotated_angle,
                        shape,
                        original_shape,
                        original_image,
                    }
                },
            ),
            embeddings: bundle.embeddings.map(|e| {
                e.into_iter()
                    .map(|e| Embedding {
                        embedding_type: e.embedding_type,
                        embedding_version: e.embedding_version,
                        embedding_inference_backend: e.embedding_inference_backend,
                    })
                    .collect()
            }),
            inference_backend: bundle.inference_backend,
        }
    }
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Thumbnail {
    pub border: Option<(f64, f64, f64, f64)>,
    pub bounding_box: Option<BBox>,
    pub rotated_angle: Option<f64>,
    pub shape: Option<(u64, u64, u64)>,
    pub original_shape: Option<(u64, u64, u64)>,
    pub original_image: Option<String>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Embedding {
    pub embedding_type: String,
    pub embedding_version: String,
    pub embedding_inference_backend: String,
}

#[derive(Serialize, JsonSchema)]
pub struct DebugReport {
    signup_id: SignupId,
    version: String,
    metadata: Metadata,
    pipeline_errors: PipelineErrors,
    sensor: SensorData,
    hardware_component_config: HardwareComponentConfig,
    tof2d: Vec<Tof2dConfig>,
    internal_state_data: InternalStateData,
    self_custody_bundle: Option<Bundle>,
    // Don't move these fields inside the Metadata or nest them, as the AI Team is specially handling long
    // time-series. @tbszlg will be mad at you!
    rgb_camera: Vec<RgbCameraMetadata>,
    ir_camera: Vec<IrCameraMetadata>,
    ir_face_camera: Vec<IrFaceCameraMetadata>,
    thermal_camera: Vec<ThermalCameraMetadata>,
    self_custody_camera: Vec<SelfCustodyRgbCameraMetadata>,
}

#[derive(Clone, Default, Serialize, JsonSchema)]
pub struct PipelineErrors {
    iris_model_error: Option<PyError>,
    occlusion_error: Option<PyError>,
    face_identifier_error: Option<PyError>,
}

/// Feedback messages to forward to the app after capture.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub enum AfterCaptureFeedbackMessage {
    /// Failure feeedback message from biometric pipeline.
    Pipeline(PipelineFailureFeedbackMessage),
    /// Server error
    ServerError,
}

#[derive(Default, PartialEq, Clone, Serialize, JsonSchema)]
pub enum SignupStatus {
    Success,
    /// Fraud detected at the orb.
    Fraud,
    /// Failure the orb. This includes pipeline crashes.
    OrbFailure,
    /// Failure from the backend. This includes duplicate signups or fraud detected at the backend.
    ServerFailure,
    /// Failure due to Orb Relay.
    OrbRelayFailure,
    /// Failure because the App is incompatible with the current Orb.
    AppIncompatible,
    /// Failure due to uninitialized state of this struct. If we see this, that's a bug.
    #[default]
    InternalError,
}

#[derive(Debug, Clone, Default)]
pub struct LocationData {
    /// The operator's team country.
    pub operator_team_operating_country: String,
    /// The operator's coordinates during the session.
    pub operator_session_coordinates: Coordinates,
    /// The operator's expected location coordinates.
    pub operator_stationary_location_coordinates: Option<Coordinates>,
    /// The orb's expected operating country.
    pub operation_country: Option<String>,
    /// The orb's expected operating city.
    pub operation_city: Option<String>,
    /// The orb's IP geolocalisation country.
    pub ip_country: Option<String>,
    /// The orb's IP geolocalisation city.
    pub ip_city: Option<String>,
}

impl LocationData {
    #[must_use]
    pub fn new(
        operation_country: Option<String>,
        operation_city: Option<String>,
        operator_location_data: operator_status::LocationData,
    ) -> Self {
        if cfg!(feature = "stage") {
            Self {
                operator_team_operating_country: "DEV".to_owned(),
                operator_session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
                operator_stationary_location_coordinates: None,
                operation_country: Some("DEV".to_owned()),
                operation_city: Some("DEV".to_owned()),
                ip_country: Some("DEV".to_owned()),
                ip_city: Some("DEV".to_owned()),
            }
        } else {
            Self {
                operator_team_operating_country: operator_location_data.team_operating_country,
                operator_session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
                operator_stationary_location_coordinates: operator_location_data
                    .stationary_location_coordinates,
                operation_country,
                operation_city,
                ip_country: ip_geo_info("country-cache"),
                ip_city: ip_geo_info("city-cache"),
            }
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            operator_team_operating_country: String::new(),
            operator_session_coordinates: Coordinates { latitude: 0.0f64, longitude: 0.0f64 },
            operator_stationary_location_coordinates: None,
            operation_country: None,
            operation_city: None,
            ip_country: None,
            ip_city: None,
        }
    }
}

#[derive(Clone, Derivative)]
#[derivative(Default)]
pub struct Builder {
    #[derivative(Default(value = "SystemTime::now()"))]
    pub start_timestamp: SystemTime,
    pub signup_id: SignupId,
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub user_qr_data: backend::user_status::UserData,
    pub signup_extension_config: Option<SignupExtensionConfig>,
    biometric_capture_succeeded: bool,
    pub signup_status: Option<SignupStatus>,
    pub enrollment_status: Option<enroll_user::Status>,
    extension_report: Option<ExtensionReport>,
    identification_images: Option<IdentificationImages>,
    rgb_net_left: Option<rgb_net::EstimateOutput>,
    rgb_net_right: Option<rgb_net::EstimateOutput>,
    pub fraud_check_results: FraudCheckResults,
    iris_model_metadata_left: Option<iris::Metadata>,
    iris_model_metadata_right: Option<iris::Metadata>,
    pipeline_errors: PipelineErrors,
    mega_agent_one_config: Option<mega_agent_one::MegaAgentOne>,
    mega_agent_two_config: Option<mega_agent_two::MegaAgentTwo>,
    biometric_capture_gps_location: Option<(f64, f64)>,
    hardware_component_config: HardwareComponentConfig,
    internal_state_data: InternalStateData,
    rgb_camera: Vec<RgbCameraMetadata>,
    ir_camera: Vec<IrCameraMetadata>,
    ir_face_camera: Vec<IrFaceCameraMetadata>,
    thermal_camera: Vec<ThermalCameraMetadata>,
    self_custody_camera: Vec<SelfCustodyRgbCameraMetadata>,
    self_custody_bundle: Option<Bundle>,
    pub self_custody_thumbnail: Option<camera::rgb::Frame>,
    pub left_iris_normalized_image: Option<NormalizedIris>,
    pub right_iris_normalized_image: Option<NormalizedIris>,
    pub left_iris_normalized_image_resized: Option<NormalizedIris>,
    pub right_iris_normalized_image_resized: Option<NormalizedIris>,
    pub identification_image_ids: Option<image_notary::IdentificationImages>,
    pub location_data: LocationData,
    pub failure_feedback_capture: Vec<CaptureFailureFeedbackMessage>,
    pub failure_feedback_after_capture: Vec<AfterCaptureFeedbackMessage>,
}

impl Builder {
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub fn build(self, end_timestamp: SystemTime, backend_config: Config) -> DebugReport {
        let Self {
            start_timestamp,
            signup_id,
            operator_qr_code,
            #[cfg(feature = "internal-data-acquisition")]
            user_qr_code,
            #[cfg(not(feature = "internal-data-acquisition"))]
                user_qr_code: _,
            user_qr_data: _,
            signup_extension_config,
            biometric_capture_succeeded,
            signup_status,
            enrollment_status,
            extension_report,
            identification_images,
            rgb_net_left,
            rgb_net_right,
            fraud_check_results,
            iris_model_metadata_left,
            iris_model_metadata_right,
            pipeline_errors,
            mega_agent_one_config,
            mega_agent_two_config,
            biometric_capture_gps_location,
            hardware_component_config,
            internal_state_data,
            rgb_camera,
            ir_camera,
            ir_face_camera,
            thermal_camera,
            self_custody_camera,
            self_custody_bundle,
            self_custody_thumbnail: _,
            left_iris_normalized_image: _,
            right_iris_normalized_image: _,
            left_iris_normalized_image_resized: _,
            right_iris_normalized_image_resized: _,
            identification_image_ids: _,
            location_data,
            failure_feedback_capture,
            failure_feedback_after_capture,
        } = self;
        let (is_self_serve, self_serve_biometric_capture_timeout) =
            (backend_config.self_serve, backend_config.self_serve_biometric_capture_timeout);
        let mut signup_extensions = Vec::new();
        if let Some(SignupExtensionConfig { mode, parameters: _ }) = &signup_extension_config {
            match mode {
                qr_scan::user::SignupMode::PupilContractionExtension => {
                    signup_extensions.push("pupil_contraction");
                }
                qr_scan::user::SignupMode::FocusSweepExtension => {
                    signup_extensions.push("focus_sweep");
                }
                qr_scan::user::SignupMode::MirrorSweepExtension => {
                    signup_extensions.push("mirror_sweep");
                }
                qr_scan::user::SignupMode::MultiWavelength => {
                    signup_extensions.push("multi_wavelength");
                }
                qr_scan::user::SignupMode::Overcapture => {
                    signup_extensions.push("overcapture");
                }
                qr_scan::user::SignupMode::Basic => {}
            }
        }
        let metadata = Metadata {
            start_timestamp: start_timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
            end_timestamp: end_timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
            biometric_capture_succeeded,
            signup_status: signup_status.unwrap_or_default(),
            enrollment_status,
            // TODO: use feature flags here
            hardware: HardwareVersion {
                front_pcb_version: "unimplemented".to_string(),
                main_board_version: "unimplemented".to_string(),
                microcontroller_version: "unimplemented".to_string(),
            },
            optics: Default::default(),
            software_version: SoftwareVersion {
                main_program: (*GIT_VERSION).clone(),
                linux_image: "unimplemented".to_string(),
                orb_os_version: ORB_OS_VERSION.clone(),
            },
            orb: OrbMetadata {
                orb_id: ORB_ID.as_str().to_owned(),
                distributor_id: operator_qr_code.user_id.to_string(),
                #[cfg(feature = "internal-data-acquisition")]
                user_coin_id: user_qr_code.user_id.to_string(),
                backend_environment: Some(format!("{:?}", *BACKEND)),
            },
            experiment_configs: ExperimentConfigs {
                extensions: signup_extensions,
                extension_report,
                is_signup_extension: signup_extension_config.is_some(),
            },
            backend_config,
            location: OrbLocation {
                ip_geolocalisation: ip_geo_info("ip-geolocalisation-cache")
                    .unwrap_or("unknown".to_owned()),
                ip_country: location_data.ip_country.unwrap_or("unknown".to_owned()),
                ip_city: location_data.ip_city.unwrap_or("unknown".to_owned()),
            },
            identification_images,
            rgb_net_left,
            rgb_net_right,
            fraud_check_results,
            software_constants: SoftwareConstants {
                sound_card_name: String::from(SOUND_CARD_NAME),
                button_shutdown_hold_time: BUTTON_LONG_PRESS_DURATION,
                status_update_interval: CONFIG_UPDATE_INTERVAL,
                qr_scan_timeout: QR_SCAN_TIMEOUT,
                detect_face_timeout: if is_self_serve {
                    DETECT_FACE_TIMEOUT_SELF_SERVE
                } else {
                    DETECT_FACE_TIMEOUT
                },
                ir_camera_default_exposure: IR_CAMERA_DEFAULT_EXPOSURE,
                ir_camera_default_gain: IR_CAMERA_DEFAULT_GAIN,
                ir_led_max_duration: IR_LED_MAX_DURATION.into(),
                ir_camera_frame_rate: IR_CAMERA_FRAME_RATE,
                default_ir_led_wavelength: DEFAULT_IR_LED_WAVELENGTH,
                default_ir_led_duration: DEFAULT_IR_LED_DURATION,
                extra_ir_led_wavelengths: EXTRA_IR_LED_WAVELENGTHS.to_vec(),
                rgb_native_width: RGB_NATIVE_WIDTH,
                rgb_native_height: RGB_NATIVE_HEIGHT,
                rgb_default_width: RGB_DEFAULT_WIDTH,
                rgb_default_height: RGB_DEFAULT_HEIGHT,
                rgb_reduced_width: RGB_REDUCED_WIDTH,
                rgb_reduced_height: RGB_REDUCED_HEIGHT,
                rgb_exposure_range: RGB_EXPOSURE_RANGE,
                rgb_fps: RGB_FPS,
                thermal_width: THERMAL_WIDTH as u16,
                thermal_height: THERMAL_HEIGHT as u16,
                user_led_default_brightness: USER_LED_DEFAULT_BRIGHTNESS,
                autofocus_min: AUTOFOCUS_MIN,
                autofocus_max: AUTOFOCUS_MAX,
                iris_sharpness_min: IRIS_SHARPNESS_MIN,
                iris_score_min: IRIS_SCORE_MIN,
                num_sharp_ir_frames: NUM_SHARP_IR_FRAMES,
                ir_focus_distance: IR_FOCUS_DISTANCE,
                ir_focus_range: [*IR_FOCUS_RANGE.start(), *IR_FOCUS_RANGE.end()],
                ir_focus_range_small: [*IR_FOCUS_RANGE_SMALL.start(), *IR_FOCUS_RANGE_SMALL.end()],
                ir_eye_save_fps: IR_EYE_SAVE_FPS,
                ir_face_save_fps: IR_FACE_SAVE_FPS,
                rgb_save_fps: RGB_SAVE_FPS,
                thermal_save_fps: THERMAL_SAVE_FPS,
                ir_voice_time_interval: IR_VOICE_TIME_INTERVAL,
                continuous_calibration_reducer: CONTINUOUS_CALIBRATION_REDUCER,
                first_sharp_iris_timeout: if is_self_serve {
                    self_serve_biometric_capture_timeout
                } else {
                    BIOMETRIC_CAPTURE_TIMEOUT
                },
            },
            iris_model_metadata_left,
            iris_model_metadata_right,
            mega_agent_one_config,
            mega_agent_two_config,
            failure_feedback_capture,
            failure_feedback_after_capture,
        };
        let sensor = SensorData {
            orbsensor: OrbSensorData {
                gps_location: biometric_capture_gps_location.unwrap_or((0.0, 0.0)),
            },
        };
        let tof2d = Default::default();
        DebugReport {
            signup_id,
            version: DEBUG_REPORT_VERSION.to_string(),
            metadata,
            pipeline_errors,
            sensor,
            hardware_component_config,
            tof2d,
            internal_state_data,
            self_custody_bundle,
            rgb_camera,
            ir_camera,
            ir_face_camera,
            thermal_camera,
            self_custody_camera,
        }
    }

    pub fn biometric_capture_succeeded(&mut self) -> &mut Self {
        self.biometric_capture_succeeded = true;
        self
    }

    pub fn signup_successful(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::Success);
        self
    }

    pub fn signup_fraud(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::Fraud);
        self
    }

    pub fn signup_orb_failure(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::OrbFailure);
        self
    }

    pub fn signup_server_failure(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::ServerFailure);
        self.failure_feedback_after_capture.push(AfterCaptureFeedbackMessage::ServerError);
        self
    }

    pub fn signup_orb_relay_failure(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::OrbRelayFailure);
        if self.enrollment_status.is_some() {
            tracing::error!("Don't use this call after enrollment_status registration");
        }
        self.enrollment_status = Some(enroll_user::Status::Error);
        self
    }

    pub fn signup_app_incompatible_failure(&mut self) -> &mut Self {
        self.signup_status = Some(SignupStatus::AppIncompatible);
        if self.enrollment_status.is_some() {
            tracing::error!("Don't use this call after enrollment_status registration");
        }
        self.enrollment_status = Some(enroll_user::Status::Error);
        self
    }

    pub fn enrollment_status(&mut self, status: enroll_user::Status) -> &mut Self {
        self.enrollment_status = Some(status);
        self
    }

    pub fn extension_report(&mut self, report: ExtensionReport) -> &mut Self {
        self.extension_report = Some(report);
        self
    }

    pub fn insert_identification_images(
        &mut self,
        identification_images: image_notary::IdentificationImages,
    ) -> &mut Self {
        self.identification_image_ids = Some(identification_images.clone());
        let image_notary::IdentificationImages {
            left_ir,
            left_ir_940nm,
            left_ir_740nm,
            right_ir,
            right_ir_940nm,
            right_ir_740nm,
            left_rgb,
            left_rgb_fullres,
            right_rgb,
            right_rgb_fullres,
            self_custody_candidate,
        } = identification_images;
        let i = self.identification_images.get_or_insert_with(Default::default);
        i.left_ir = left_ir;
        i.left_ir_940nm = left_ir_940nm;
        i.left_ir_740nm = left_ir_740nm;
        i.right_ir = right_ir;
        i.right_ir_940nm = right_ir_940nm;
        i.right_ir_740nm = right_ir_740nm;
        i.left_rgb = left_rgb;
        i.left_rgb_fullres = left_rgb_fullres;
        i.right_rgb = right_rgb;
        i.right_rgb_fullres = right_rgb_fullres;
        i.self_custody_candidate = self_custody_candidate;
        self
    }

    pub fn fraud_check_report(&mut self, report: fraud_check::Report) -> &mut Self {
        self.fraud_check_results.report = Some(report);
        self
    }

    pub fn fraud_check_feedback_messages(
        &mut self,
        messages: &[PipelineFailureFeedbackMessage],
    ) -> &mut Self {
        self.failure_feedback_after_capture
            .extend(messages.iter().map(|msg| AfterCaptureFeedbackMessage::Pipeline(msg.clone())));
        self
    }

    pub fn face_identifier_results(
        &mut self,
        checks: Result<face_identifier::FraudChecks, PyError>,
    ) -> &mut Self {
        match checks {
            Ok(t) => self.fraud_check_results.face_identifier_checks = Some(t),
            Err(e) => self.pipeline_errors.face_identifier_error = Some(e),
        }
        self
    }

    pub fn occlusion_error(&mut self, error: Option<PyError>) {
        self.pipeline_errors.occlusion_error = error;
    }

    pub fn iris_model_metadata(
        &mut self,
        eye_left: iris::Metadata,
        eye_right: iris::Metadata,
    ) -> &mut Self {
        self.iris_model_metadata_left = Some(eye_left);
        self.iris_model_metadata_right = Some(eye_right);
        self
    }

    pub fn iris_model_error(&mut self, error: Option<PyError>) -> &mut Self {
        self.pipeline_errors.iris_model_error = error;
        self.failure_feedback_after_capture.push(AfterCaptureFeedbackMessage::Pipeline(
            PipelineFailureFeedbackMessage::EyesOcclusion,
        ));
        self
    }

    pub fn mega_agent_one_config(
        &mut self,
        mega_agent_one_config: mega_agent_one::MegaAgentOne,
    ) -> &mut Self {
        self.mega_agent_one_config = Some(mega_agent_one_config);
        self
    }

    pub fn mega_agent_two_config(
        &mut self,
        mega_agent_two_config: mega_agent_two::MegaAgentTwo,
    ) -> &mut Self {
        self.mega_agent_two_config = Some(mega_agent_two_config);
        self
    }

    pub fn biometric_capture_gps_location(&mut self, latitude: f64, longitude: f64) -> &mut Self {
        self.biometric_capture_gps_location = Some((latitude, longitude));
        self
    }

    pub fn biometric_capture_history(&mut self, mut history: biometric_capture::Log) -> &mut Self {
        self.hardware_component_config = HardwareComponentConfig {
            ir_camera: IrCameraConfig {
                liquid_lens: history.main_mcu.liquid_lens.iter().copied().collect(),
                common_config: CommonCameraConfig {
                    auto_exposure: history.ir_eye_camera.exposure.iter().copied().collect(),
                    auto_gain: history.ir_eye_camera.gain.iter().copied().collect(),
                },
            },
            ir_led: IrLedConfig {
                wavelength: history.main_mcu.ir_led.iter().copied().collect(),
                duration: history.main_mcu.ir_led_duration.iter().copied().collect(),
                ..Default::default()
            },
            led: WhiteLedConfig {
                brightness: history.main_mcu.user_led_brightness.iter().copied().collect(),
            },
            mirror: MirrorConfigDegrees {
                left_eye_phi: history.mirror.phi_degrees.iter().copied().collect(),
                left_eye_theta: history.mirror.theta_degrees.iter().copied().collect(),
                right_eye_phi: Vec::new(),
                right_eye_theta: Vec::new(),
            },
            ..Default::default()
        };
        self.internal_state_data.user_distance =
            history.user_distance.user_distance.iter().copied().collect();

        self
    }

    pub fn biometric_capture_feedback_messages(
        &mut self,
        messages: Vec<CaptureFailureFeedbackMessage>,
    ) -> &mut Self {
        self.failure_feedback_capture = messages;
        self
    }

    pub fn image_notary_history(&mut self, mut image_notary: image_notary::Log) -> &mut Self {
        self.rgb_camera = (&mut image_notary.rgb_net_metadata).into();
        self.ir_camera = (&mut image_notary.ir_net_metadata).into();
        self.ir_face_camera = (&mut image_notary.ir_face_metadata).into();
        self.thermal_camera = (&mut image_notary.thermal_metadata).into();
        self.self_custody_camera = (&mut image_notary.face_identifier_metadata).into();
        self
    }

    pub fn rgb_net_metadata(
        &mut self,
        left: rgb_net::EstimateOutput,
        right: rgb_net::EstimateOutput,
    ) -> &mut Self {
        self.rgb_net_left = Some(left);
        self.rgb_net_right = Some(right);
        self
    }

    pub fn self_custody_bundle(&mut self, bundle: Option<FIBundle>) -> &mut Self {
        self.self_custody_bundle = bundle.map(Into::into);
        self
    }

    pub fn self_custody_thumbnail(&mut self, bundle: Option<FIBundle>) -> &mut Self {
        if let Some(frame) = bundle
            .and_then(|b| b.thumbnail)
            .and_then(|t| t.image)
            .and_then(std::convert::Into::into)
        {
            self.self_custody_thumbnail = Some(frame.into());
        }
        self
    }

    pub fn iris_normalized_images(
        &mut self,
        left: Option<NormalizedIris>,
        right: Option<NormalizedIris>,
        left_resized: Option<NormalizedIris>,
        right_resized: Option<NormalizedIris>,
    ) -> &mut Self {
        self.left_iris_normalized_image = left;
        self.right_iris_normalized_image = right;
        self.left_iris_normalized_image_resized = left_resized;
        self.right_iris_normalized_image_resized = right_resized;
        self
    }

    pub fn operator_id_age_verification(
        &mut self,
        operator_id_age_verification: String,
    ) -> &mut Self {
        self.fraud_check_results.operator_id_age_verification = Some(operator_id_age_verification);
        self
    }

    #[must_use]
    pub fn failure_feedback_capture_proto(&self) -> Vec<i32> {
        self.failure_feedback_capture.iter().map(|msg| match *msg {
            CaptureFailureFeedbackMessage::FaceOcclusionOrPoorLighting => self_serve::orb::v1::capture_ended::FailureFeedbackType::FaceOcclusionOrPoorLighting,
            CaptureFailureFeedbackMessage::TooFar => self_serve::orb::v1::capture_ended::FailureFeedbackType::TooFar,
            CaptureFailureFeedbackMessage::TooClose => self_serve::orb::v1::capture_ended::FailureFeedbackType::TooClose,
            CaptureFailureFeedbackMessage::EyesOcclusion => self_serve::orb::v1::capture_ended::FailureFeedbackType::EyesOcclusion,
        } as i32).collect()
    }

    #[must_use]
    pub fn failure_feedback_after_capture_proto(&self) -> Vec<i32> {
        let mut messages: Vec<i32> = self
            .failure_feedback_after_capture
            .iter()
            .map(|msg| match msg {
                // We don't disclose the full list of reasons because many of them relate to fraud.
                // Instead this list only contains a high level reason which helps end-users
                // troubleshoot.
                AfterCaptureFeedbackMessage::Pipeline(msg) => match msg {
                    PipelineFailureFeedbackMessage::ContactLenses => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::ContactLenses
                    }
                    PipelineFailureFeedbackMessage::EyeGlasses => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::EyeGlasses
                    }
                    PipelineFailureFeedbackMessage::Mask => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::Mask
                    }
                    PipelineFailureFeedbackMessage::FaceOcclusion => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::FaceOcclusion
                    }
                    PipelineFailureFeedbackMessage::MultipleFaces => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::MultipleFaces
                    }
                    PipelineFailureFeedbackMessage::EyesOcclusion => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::EyesOcclusion
                    }
                    PipelineFailureFeedbackMessage::HeadPose => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::HeadPose
                    }
                    PipelineFailureFeedbackMessage::Underaged => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::Underaged
                    }
                    PipelineFailureFeedbackMessage::LowImageQuality => {
                        self_serve::orb::v1::signup_ended::FailureFeedbackType::LowImageQuality
                    }
                },
                AfterCaptureFeedbackMessage::ServerError => {
                    self_serve::orb::v1::signup_ended::FailureFeedbackType::ServerError
                }
            } as i32)
            .collect();
        messages.sort_unstable();
        messages.dedup();
        messages
    }
}

impl DebugReport {
    #[must_use]
    pub fn builder(
        start_timestamp: SystemTime,
        signup_id: &SignupId,
        qr_codes: &plans::ResolvedQrCodes,
        backend_config: Config,
    ) -> Builder {
        let plans::ResolvedQrCodes {
            operator_data,
            user_qr_code,
            user_data,
            user_qr_code_string: _,
        } = qr_codes;
        let combined_signup_extension_config = user_qr_code
            .signup_extension_config
            .as_ref()
            .or(operator_data.qr_code.signup_extension_config.as_ref())
            .cloned();
        Builder {
            start_timestamp,
            signup_id: signup_id.clone(),
            operator_qr_code: operator_data.qr_code.clone(),
            user_qr_code: user_qr_code.clone(),
            user_qr_data: user_data.clone(),
            signup_extension_config: combined_signup_extension_config,
            biometric_capture_succeeded: false,
            signup_status: None,
            enrollment_status: None,
            extension_report: None,
            identification_images: None,
            rgb_net_left: None,
            rgb_net_right: None,
            fraud_check_results: FraudCheckResults::default(),
            iris_model_metadata_left: None,
            iris_model_metadata_right: None,
            pipeline_errors: PipelineErrors::default(),
            mega_agent_one_config: None,
            mega_agent_two_config: None,
            biometric_capture_gps_location: None,
            hardware_component_config: HardwareComponentConfig::default(),
            internal_state_data: InternalStateData::default(),
            rgb_camera: Vec::new(),
            ir_camera: Vec::new(),
            ir_face_camera: Vec::new(),
            thermal_camera: Vec::new(),
            self_custody_camera: Vec::new(),
            self_custody_bundle: None,
            self_custody_thumbnail: None,
            left_iris_normalized_image: None,
            right_iris_normalized_image: None,
            left_iris_normalized_image_resized: None,
            right_iris_normalized_image_resized: None,
            identification_image_ids: None,
            location_data: LocationData::new(
                backend_config.operation_country,
                backend_config.operation_city,
                operator_data.location_data.clone(),
            ),
            failure_feedback_after_capture: Vec::new(),
            failure_feedback_capture: Vec::new(),
        }
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(ImageId, Option<IrNetMetadata>, IrLed, bool, Duration, bool)>>
    for Vec<IrCameraMetadata>
{
    fn from(
        ir_net_metadata_history: &mut TimeSeries<(
            ImageId,
            Option<IrNetMetadata>,
            IrLed,
            bool,
            Duration,
            bool,
        )>,
    ) -> Self {
        ir_net_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value:
                        (
                            image_id,
                            ir_net_metadata,
                            wavelength,
                            target_left_eye,
                            capture_timestamp,
                            saved,
                        ),
                    timestamp,
                } = value;
                IrCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                        *saved,
                    ),
                    // TODO: Check this is the correct convention!
                    side: if *target_left_eye { Some(1) } else { Some(0) },
                    wavelength: *wavelength,
                    irnet: ir_net_metadata.clone(),
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(ImageId, IrLed, Duration, bool)>> for Vec<IrFaceCameraMetadata> {
    fn from(
        ir_face_camera_metadata_history: &mut TimeSeries<(ImageId, IrLed, Duration, bool)>,
    ) -> Self {
        ir_face_camera_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, wavelength, capture_timestamp, saved),
                    timestamp,
                } = value;
                IrFaceCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                        *saved,
                    ),
                    wavelength: *wavelength,
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(ImageId, rgb_net::EstimateOutput, Duration, bool)>>
    for Vec<RgbCameraMetadata>
{
    fn from(
        rgb_net_metadata_history: &mut TimeSeries<(
            ImageId,
            rgb_net::EstimateOutput,
            Duration,
            bool,
        )>,
    ) -> Self {
        rgb_net_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, rgb_net_metadata, capture_timestamp, saved),
                    timestamp,
                } = value;
                RgbCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                        *saved,
                    ),
                    rgbnet: Some(rgb_net_metadata.clone()),
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(ImageId, FaceIdentifierIsValidMetadata, Duration, bool)>>
    for Vec<SelfCustodyRgbCameraMetadata>
{
    fn from(
        face_identifier_metadata: &mut TimeSeries<(
            ImageId,
            FaceIdentifierIsValidMetadata,
            Duration,
            bool,
        )>,
    ) -> Self {
        face_identifier_metadata
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, face_identifier_metadata, capture_timestamp, saved),
                    timestamp,
                } = value;
                SelfCustodyRgbCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                        *saved,
                    ),
                    face_identifier_metadata: face_identifier_metadata.clone(),
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(ImageId, IrLed, Duration, bool)>> for Vec<ThermalCameraMetadata> {
    fn from(
        thermal_camera_metadata_history: &mut TimeSeries<(ImageId, IrLed, Duration, bool)>,
    ) -> Self {
        thermal_camera_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, wavelength, capture_timestamp, saved),
                    timestamp,
                } = value;
                ThermalCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                        *saved,
                    ),
                    wavelength: *wavelength,
                }
            })
            .collect()
    }
}

impl CommonImageMetadata {
    fn new(
        image_id: &ImageId,
        timestamp: Instant,
        capture_timestamp: Duration,
        image_saved: bool,
    ) -> Self {
        Self {
            image_id: image_id.to_string(),
            timestamp: Some(SerializableInstant::new(timestamp)),
            image_saved,
            optical: Default::default(),
            roi: Default::default(),
            execution_timestamp: Default::default(),
            capture_timestamp,
        }
    }
}

////////////////////////////// Metadata //////////////////////////////

#[derive(Serialize, JsonSchema)]
pub struct Metadata {
    start_timestamp: f64,
    end_timestamp: f64,
    biometric_capture_succeeded: bool,
    signup_status: SignupStatus,
    enrollment_status: Option<enroll_user::Status>,
    hardware: HardwareVersion,
    optics: OpticsVersion,
    software_version: SoftwareVersion,
    orb: OrbMetadata,
    experiment_configs: ExperimentConfigs,
    backend_config: Config,
    identification_images: Option<IdentificationImages>,
    rgb_net_left: Option<rgb_net::EstimateOutput>,
    rgb_net_right: Option<rgb_net::EstimateOutput>,
    fraud_check_results: FraudCheckResults,
    software_constants: SoftwareConstants,
    location: OrbLocation,
    iris_model_metadata_left: Option<iris::Metadata>,
    iris_model_metadata_right: Option<iris::Metadata>,
    mega_agent_one_config: Option<mega_agent_one::MegaAgentOne>,
    mega_agent_two_config: Option<mega_agent_two::MegaAgentTwo>,
    failure_feedback_capture: Vec<CaptureFailureFeedbackMessage>,
    failure_feedback_after_capture: Vec<AfterCaptureFeedbackMessage>,
}

#[derive(Clone, Serialize, JsonSchema, Default)]
pub struct FraudCheckResults {
    pub report: Option<fraud_check::Report>,
    face_identifier_checks: Option<face_identifier::FraudChecks>,
    operator_id_age_verification: Option<String>,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize, JsonSchema, Default, Debug)]
struct HardwareVersion {
    #[serde(rename = "frontPCBVersion")]
    front_pcb_version: String,
    main_board_version: String,
    microcontroller_version: String,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize, JsonSchema, Default, Debug)]
struct OpticsVersion {
    sensor_version: String,
    autofocus_lens_version: String,
    lens_version: String,
    guiding_system_version: String,
    bandpass_filter_version: String,
    one_camera_setup_version: String,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize, JsonSchema, Default, Debug)]
struct SoftwareVersion {
    main_program: String,
    linux_image: String,
    orb_os_version: String,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct OrbMetadata {
    orb_id: String,
    distributor_id: String,
    #[cfg(feature = "internal-data-acquisition")]
    user_coin_id: String,
    backend_environment: Option<String>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct ExperimentConfigs {
    extensions: Vec<&'static str>,
    extension_report: Option<ExtensionReport>,
    is_signup_extension: bool,
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize, JsonSchema, Default, Debug)]
struct OrbLocation {
    ip_geolocalisation: String,
    ip_country: String,
    ip_city: String,
}

#[derive(Serialize, JsonSchema, Debug)]
struct SoftwareConstants {
    sound_card_name: String,
    button_shutdown_hold_time: Duration,
    status_update_interval: Duration,
    qr_scan_timeout: Duration,
    detect_face_timeout: Duration,
    ir_camera_default_exposure: i64,
    ir_camera_default_gain: i64,
    ir_led_max_duration: i64,
    ir_camera_frame_rate: u16,
    default_ir_led_wavelength: IrLed,
    default_ir_led_duration: u16,
    extra_ir_led_wavelengths: Vec<(IrLed, u16)>,
    rgb_native_width: u32,
    rgb_native_height: u32,
    rgb_default_width: u32,
    rgb_default_height: u32,
    rgb_reduced_width: u32,
    rgb_reduced_height: u32,
    rgb_exposure_range: RangeInclusive<u32>,
    rgb_fps: u32,
    thermal_width: u16,
    thermal_height: u16,
    user_led_default_brightness: u8,
    autofocus_min: i16,
    autofocus_max: i16,
    iris_sharpness_min: f64,
    iris_score_min: f64,
    num_sharp_ir_frames: usize,
    ir_focus_distance: f64,
    ir_focus_range: [f64; 2],
    ir_focus_range_small: [f64; 2],
    ir_eye_save_fps: f32,
    ir_face_save_fps: f32,
    rgb_save_fps: f32,
    thermal_save_fps: f32,
    ir_voice_time_interval: Duration,
    continuous_calibration_reducer: f64,
    first_sharp_iris_timeout: Duration,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
pub struct IdentificationImages {
    pub left_ir: ImageId,
    pub left_ir_940nm: Option<ImageId>,
    pub left_ir_740nm: Option<ImageId>,
    pub right_ir: ImageId,
    pub right_ir_940nm: Option<ImageId>,
    pub right_ir_740nm: Option<ImageId>,
    pub left_rgb: ImageId,
    pub left_rgb_fullres: ImageId,
    pub right_rgb: ImageId,
    pub right_rgb_fullres: ImageId,
    pub self_custody_candidate: ImageId,
}

////////////////////////////// SensorData //////////////////////////////

#[derive(Serialize, JsonSchema, Default, Debug)]
struct SensorData {
    orbsensor: OrbSensorData,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct OrbSensorData {
    gps_location: (f64, f64),
}

////////////////////////////// HardwareComponentConfig //////////////////////////////

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct HardwareComponentConfig {
    ir_camera: IrCameraConfig,
    rgb_camera: RgbCameraConfig,
    front_ir_camera: FrontIrCameraConfig,
    heat_camera: HeatCameraConfig,
    tof2d: Tof2dConfig,
    ir_led: IrLedConfig,
    led: WhiteLedConfig,
    mirror: MirrorConfigDegrees,
    voice: VoiceConfig,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct IrCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    liquid_lens: Vec<Timestamped<Option<i16>>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct RgbCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    shutter: Vec<()>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct FrontIrCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct HeatCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct Tof2dConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    mode: Vec<()>,
    distance_filter: Vec<()>,
    filter_level: Vec<()>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct CommonCameraConfig {
    auto_exposure: Vec<Timestamped<i64>>,
    auto_gain: Vec<Timestamped<i64>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct IrLedConfig {
    is_on: Vec<bool>,
    wavelength: Vec<Timestamped<IrLed>>,
    duration: Vec<Timestamped<u16>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct WhiteLedConfig {
    brightness: Vec<Timestamped<u8>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct MirrorConfigDegrees {
    left_eye_phi: Vec<Timestamped<f64>>,
    left_eye_theta: Vec<Timestamped<f64>>,
    right_eye_phi: Vec<Timestamped<f64>>,
    right_eye_theta: Vec<Timestamped<f64>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct VoiceConfig {
    voice: Vec<String>,
}

////////////////////////////// RgbCameraMetadata //////////////////////////////

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct RgbCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    rgbnet: Option<rgb_net::EstimateOutput>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
pub struct CommonImageMetadata {
    image_id: String,
    timestamp: Option<SerializableInstant>,
    image_saved: bool,
    optical: CommonOpticalMetadata,
    roi: CommonCameraRoi,
    execution_timestamp: CommonCameraExecutionTimestamp,
    capture_timestamp: Duration,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct CommonOpticalMetadata {
    gain: Option<i64>,
    exposure: Option<u8>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct Coordinate {
    x: Option<u16>,
    y: Option<u16>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct CommonCameraRoi {
    top_left_coordinate: Coordinate,
    bottom_right_coordinate: Coordinate,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct CommonCameraExecutionTimestamp {
    timestamp_processed_by_autofocus: Option<f64>,
    timestamp_processed_by_guiding_system: Option<f64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FaceIdentifierIsValidMetadata {
    pub is_valid: Option<bool>,
    pub score: Option<f64>,
    pub inference_backend: Option<String>,
    pub error: Option<PyError>,
    pub rgb_net_eye_landmarks: (rgb_net::Point, rgb_net::Point),
    pub rgb_net_bbox: rgb_net::Rectangle,
}

impl From<face_identifier::types::IsValidOutput> for FaceIdentifierIsValidMetadata {
    fn from(v: face_identifier::types::IsValidOutput) -> Self {
        Self {
            is_valid: v.is_valid,
            score: v.score,
            inference_backend: v.inference_backend,
            error: v.error,
            rgb_net_eye_landmarks: v.rgb_net_eye_landmarks,
            rgb_net_bbox: v.rgb_net_bbox,
        }
    }
}

////////////////////////////// IrCameraMetadata //////////////////////////////

#[derive(Clone, Serialize, JsonSchema)]
pub struct IrCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    side: Option<u8>,
    wavelength: IrLed,
    irnet: Option<IrNetMetadata>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize, JsonSchema, Default, Debug, Clone)]
pub struct IrNetMetadata {
    #[schemars(with = "Option<Vec<Vec<f32>>>")]
    landmarks: Option<Array2<f32>>,
    fractional_sharpness_score: f64,
    occlusion_30: f64,
    occlusion_90: f64,
    pupil_to_iris_ratio: f64,
    gaze: f64,
    eye_detected: f64,
    qr_code_detected: f64,
    occlusion_30_old: f64,
    eye_opened: bool,
    iris_aligned: bool,
    iris_sharp: bool,
    iris_uncovered: bool,
    orientation_correct: bool,
    gaze_valid: bool,
    valid_for_identification: bool,
    status: i64,
    selection_score: f64,
    msg: String,
    mean_brightness_raw: f64,
    target_side: u8,
    perceived_side: Option<i32>,
}

impl From<ir_net::EstimateOutput> for IrNetMetadata {
    fn from(ir_net_estimate: ir_net::EstimateOutput) -> Self {
        let ir_net::EstimateOutput {
            landmarks,
            sharpness,
            occlusion_30,
            occlusion_90,
            pupil_to_iris_ratio,
            gaze,
            eye_detected,
            qr_code_detected,
            occlusion_30_old,
            eye_opened,
            iris_aligned,
            iris_sharp,
            iris_uncovered,
            orientation_correct,
            gaze_valid,
            valid_for_identification,
            status,
            message,
            score,
            mean_brightness_raw,
            target_side,
            perceived_side,
        } = ir_net_estimate;
        Self {
            landmarks: landmarks.map(RkyvNdarray::<_, Ix2>::into_ndarray),
            fractional_sharpness_score: sharpness,
            occlusion_30,
            occlusion_90,
            pupil_to_iris_ratio,
            gaze,
            eye_detected,
            qr_code_detected,
            occlusion_30_old,
            eye_opened,
            iris_aligned,
            iris_sharp,
            iris_uncovered,
            orientation_correct,
            gaze_valid,
            valid_for_identification,
            status,
            selection_score: score,
            msg: message,
            mean_brightness_raw,
            target_side,
            perceived_side,
        }
    }
}

////////////////////////////// IrFaceCameraMetadata //////////////////////////////

#[derive(Clone, Serialize, JsonSchema)]
pub struct IrFaceCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    /// Wavelength of the ir LEDs at the time of capture
    wavelength: IrLed,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct SelfCustodyRgbCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    face_identifier_metadata: FaceIdentifierIsValidMetadata,
}

////////////////////////////// HeatCameraMetadata //////////////////////////////

#[derive(Clone, Serialize, JsonSchema)]
pub struct ThermalCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    /// Wavelength of the ir LEDs at the time of capture.
    wavelength: IrLed,
}

////////////////////////////// InternalStateData //////////////////////////////

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct InternalStateData {
    autofocus: InternalStateAutofocus,
    mirror_eye_tracking_pid: MirrorEyeTrackingPid,
    user_distance: Vec<Timestamped<f64>>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct InternalStateAutofocus {
    crit_sharpness: f64,
    valid_history_delay: f64,
    latency_focus_setting: f64,
    left: AutofocusTarget,
    right: AutofocusTarget,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct AutofocusTarget {
    target: ValueOffsetTimestamp,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct ValueOffsetTimestamp {
    value: f64,
    offset: f64,
    timestamp: f64,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct MirrorEyeTrackingPid {
    left: Vec<MirrorOffsetSetting>,
    right: Vec<MirrorOffsetSetting>,
}

#[derive(Clone, Serialize, JsonSchema, Default, Debug)]
struct MirrorOffsetSetting {
    x: f64,
    y: f64,
    timestamp: f64,
}
