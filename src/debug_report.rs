//! Signup data structures.

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
    backend::{endpoints::BACKEND, user_status::UserData},
    config::Config,
    consts::{
        AUTOFOCUS_MAX, AUTOFOCUS_MIN, BIOMETRIC_CAPTURE_TIMEOUT, BUTTON_LONG_PRESS_DURATION,
        CONFIG_DIR, CONFIG_UPDATE_INTERVAL, CONTINUOUS_CALIBRATION_REDUCER,
        DEFAULT_IR_LED_DURATION, DEFAULT_IR_LED_WAVELENGTH, DETECT_FACE_TIMEOUT,
        EXTRA_IR_LED_WAVELENGTHS, IRIS_SCORE_MIN, IRIS_SHARPNESS_MIN, IR_CAMERA_DEFAULT_EXPOSURE,
        IR_CAMERA_DEFAULT_GAIN, IR_CAMERA_FRAME_RATE, IR_EYE_SAVE_FPS, IR_FACE_SAVE_FPS,
        IR_FOCUS_DISTANCE, IR_FOCUS_RANGE, IR_FOCUS_RANGE_SMALL, IR_LED_MAX_DURATION,
        IR_VOICE_TIME_INTERVAL, NUM_SHARP_IR_FRAMES, QR_SCAN_TIMEOUT, RGB_DEFAULT_HEIGHT,
        RGB_DEFAULT_WIDTH, RGB_EXPOSURE_RANGE, RGB_FPS, RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH,
        RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH, RGB_SAVE_FPS, SOUND_CARD_NAME, THERMAL_HEIGHT,
        THERMAL_SAVE_FPS, THERMAL_WIDTH, USER_LED_DEFAULT_BRIGHTNESS,
    },
    identification::{GIT_VERSION, ORB_ID, OVERALL_SOFTWARE_VERSION},
    mcu::main::IrLed,
    plans::{biometric_capture, fraud_check, qr_scan},
    serializable_instant::SerializableInstant,
    time_series::TimeSeries,
    timestamped::Timestamped,
    utils::RkyvNdarray,
};
use derivative::Derivative;
use eyre::Result;
#[cfg(test)]
use mock_instant::Instant;
use ndarray::prelude::*;
use orb_wld_data_id::{ImageId, SignupId};
use python_agent_interface::PyError;
use schemars::JsonSchema;
use serde::Serialize;
#[cfg(not(test))]
use std::time::Instant;
use std::{
    default::Default,
    fs,
    ops::RangeInclusive,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DEBUG_REPORT_VERSION: &str =
    "f70472616387d5c5f1d621358e0b79c2356e5c02d12683d3f80dc32c28742cf9";

#[derive(Serialize, JsonSchema)]
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

#[derive(Serialize, JsonSchema)]
pub struct Thumbnail {
    pub border: Option<(f64, f64, f64, f64)>,
    pub bounding_box: Option<BBox>,
    pub rotated_angle: Option<f64>,
    pub shape: Option<(u64, u64, u64)>,
    pub original_shape: Option<(u64, u64, u64)>,
    pub original_image: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Embedding {
    pub embedding_type: String,
    pub embedding_version: String,
    pub embedding_inference_backend: String,
}

#[derive(Serialize, JsonSchema)]
pub struct DebugReport {
    signup_id: SignupId,
    version: String,
    metadata: DebugMetadata,
    pipeline_errors: PipelineErrors,
    sensor: SensorData,
    hardware_component_config: HardwareComponentConfig,
    tof2d: Vec<Tof2dConfig>,
    internal_state_data: InternalStateData,
    self_custody_bundle: Option<Bundle>,
    rgb_camera: Vec<RgbCameraMetadata>,
    ir_camera: Vec<IrCameraMetadata>,
    ir_face_camera: Vec<IrFaceCameraMetadata>,
    thermal_camera: Vec<ThermalCameraMetadata>,
    self_custody_camera: Vec<SelfCustodyRgbCameraMetadata>,
}

#[derive(Default, Serialize, JsonSchema)]
pub struct PipelineErrors {
    iris_model_error: Option<PyError>,
    occlusion_error: Option<PyError>,
    contact_lens_left_error: Option<PyError>,
    contact_lens_right_error: Option<PyError>,
    person_classification_left_error: Option<PyError>,
    person_classification_right_error: Option<PyError>,
    face_identifier_error: Option<PyError>,
}

#[derive(Default, PartialEq, Clone, Serialize, JsonSchema)]
pub enum SingupStatus {
    Success,
    Failure,
    #[default]
    InternalError,
    Fraud,
}

#[derive(Derivative)]
#[derivative(Default)]
pub struct Builder {
    #[derivative(Default(value = "SystemTime::now()"))]
    pub start_timestamp: SystemTime,
    pub signup_id: SignupId,
    pub operator_qr_code: qr_scan::user::Data,
    pub user_qr_code: qr_scan::user::Data,
    pub user_data: UserData,
    biometric_capture_succeeded: bool,
    pub signup_status: SingupStatus,
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
    rgb_camera: Vec<RgbCameraMetadata>,
    ir_camera: Vec<IrCameraMetadata>,
    ir_face_camera: Vec<IrFaceCameraMetadata>,
    thermal_camera: Vec<ThermalCameraMetadata>,
    self_custody_camera: Vec<SelfCustodyRgbCameraMetadata>,
    self_custody_bundle: Option<Bundle>,
    pub self_custody_thumbnail: Option<camera::rgb::Frame>,
    pub left_iris_normalized_image: Option<NormalizedIris>,
    pub right_iris_normalized_image: Option<NormalizedIris>,
}

impl Builder {
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub fn build(self, end_timestamp: SystemTime, backend_config: Config) -> DebugReport {
        let Self {
            start_timestamp,
            signup_id,
            operator_qr_code,
            user_qr_code: _,
            user_data,
            biometric_capture_succeeded,
            signup_status,
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
            rgb_camera,
            ir_camera,
            ir_face_camera,
            thermal_camera,
            self_custody_camera,
            self_custody_bundle,
            self_custody_thumbnail: _,
            left_iris_normalized_image: _,
            right_iris_normalized_image: _,
        } = self;
        let metadata = DebugMetadata {
            start_timestamp: start_timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
            end_timestamp: end_timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
            biometric_capture_succeeded,
            signup_status,
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
                overall_software_version: OVERALL_SOFTWARE_VERSION.clone(),
            },
            orb: OrbMetadata {
                orb_id: ORB_ID.as_str().to_owned(),
                distributor_id: operator_qr_code.user_id.to_string(),
                data_policy: user_data.data_policy.to_string(),
                backend_environment: Some(format!("{:?}", *BACKEND)),
            },
            backend_config,
            location: OrbLocation {
                ip_geolocalisation: load_cache("ip-geolocalisation-cache")
                    .unwrap_or_else(|| "unknown".to_string()),
                ip_country: load_cache("country-cache").unwrap_or_else(|| "unknown".to_string()),
                ip_city: load_cache("city-cache").unwrap_or_else(|| "unknown".to_string()),
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
                detect_face_timeout: DETECT_FACE_TIMEOUT,
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
                first_sharp_iris_timeout: BIOMETRIC_CAPTURE_TIMEOUT,
            },
            iris_model_metadata_left,
            iris_model_metadata_right,
            mega_agent_one_config,
            mega_agent_two_config,
        };
        let sensor = SensorData {
            orbsensor: OrbSensorData {
                gps_location: biometric_capture_gps_location.unwrap_or((0.0, 0.0)),
            },
        };
        let tof2d = Default::default();
        let internal_state_data = Default::default();
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
        self.signup_status = SingupStatus::Success;
        self
    }

    pub fn signup_fraud(&mut self) -> &mut Self {
        self.signup_status = SingupStatus::Fraud;
        self
    }

    pub fn signup_failure(&mut self) -> &mut Self {
        self.signup_status = SingupStatus::Failure;
        self
    }

    pub fn identification_images(
        &mut self,
        identification_images: image_notary::IdentificationImages,
    ) -> &mut Self {
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

    pub fn insert_self_custody_thumbnail_id(
        &mut self,
        self_custody_thumbnail: ImageId,
    ) -> &mut Self {
        let i = self.identification_images.get_or_insert_with(Default::default);
        i.self_custody_thumbnail = Some(self_custody_thumbnail);
        self
    }

    pub fn insert_iris_normalized_image_ids(
        &mut self,
        left_iris_normalized_image: Option<ImageId>,
        left_iris_normalized_mask: Option<ImageId>,
        right_iris_normalized_image: Option<ImageId>,
        right_iris_normalized_mask: Option<ImageId>,
    ) -> Result<&mut Self> {
        let i = self.identification_images.get_or_insert_with(Default::default);
        i.left_iris_normalized_image = left_iris_normalized_image;
        i.left_iris_normalized_mask = left_iris_normalized_mask;
        i.right_iris_normalized_image = right_iris_normalized_image;
        i.right_iris_normalized_mask = right_iris_normalized_mask;
        Ok(self)
    }

    pub fn fraud_check_report(&mut self, report: fraud_check::Report) -> &mut Self {
        self.fraud_check_results.report = Some(report);
        self
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
            mirror: MirrorConfig {
                left_horizontal: history.mirror.horizontal.iter().copied().collect(),
                left_vertical: history.mirror.vertical.iter().copied().collect(),
                right_horizontal: Vec::new(),
                right_vertical: Vec::new(),
            },
            ..Default::default()
        };
        self
    }

    pub fn image_notary_history(&mut self, mut log: image_notary::Log) -> &mut Self {
        self.rgb_camera = (&mut log.rgb_net_metadata).into();
        self.ir_camera = (&mut log.ir_net_metadata).into();
        self.ir_face_camera = (&mut log.ir_face_metadata).into();
        self.thermal_camera = (&mut log.thermal_metadata).into();
        self.self_custody_camera = (&mut log.face_identifier_metadata).into();
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
    ) -> &mut Self {
        self.left_iris_normalized_image = left;
        self.right_iris_normalized_image = right;
        self
    }
}

impl DebugReport {
    #[must_use]
    pub fn builder(
        start_timestamp: SystemTime,
        signup_id: &SignupId,
        operator_qr_code: &qr_scan::user::Data,
        user_qr_code: &qr_scan::user::Data,
        user_data: &UserData,
    ) -> Builder {
        Builder {
            start_timestamp,
            signup_id: signup_id.clone(),
            operator_qr_code: operator_qr_code.clone(),
            user_qr_code: user_qr_code.clone(),
            user_data: user_data.clone(),
            biometric_capture_succeeded: false,
            signup_status: SingupStatus::Failure,
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
            rgb_camera: Vec::new(),
            ir_camera: Vec::new(),
            ir_face_camera: Vec::new(),
            thermal_camera: Vec::new(),
            self_custody_camera: Vec::new(),
            self_custody_bundle: None,
            self_custody_thumbnail: None,
            left_iris_normalized_image: None,
            right_iris_normalized_image: None,
        }
    }
}

fn load_cache(cache_file_name: &str) -> Option<String> {
    let path = Path::new(CONFIG_DIR).join(cache_file_name);
    if !path.exists() {
        tracing::warn!("Config file at {} not exists", path.display());
        return None;
    }
    tracing::info!("Loading cached data from {}", path.display());
    let contents = fs::read_to_string(path);
    tracing::debug!("Cached file contents: {contents:#?}");
    Some(contents.unwrap_or_else(|_| "unknown".to_string()))
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(Option<ImageId>, Option<IrNetMetadata>, IrLed, bool, Duration)>>
    for Vec<IrCameraMetadata>
{
    fn from(
        ir_net_metadata_history: &mut TimeSeries<(
            Option<ImageId>,
            Option<IrNetMetadata>,
            IrLed,
            bool,
            Duration,
        )>,
    ) -> Self {
        ir_net_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value:
                        (image_id, ir_net_metadata, wavelength, target_left_eye, capture_timestamp),
                    timestamp,
                } = value;
                IrCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
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
impl From<&'_ mut TimeSeries<(Option<ImageId>, IrLed, Duration)>> for Vec<IrFaceCameraMetadata> {
    fn from(
        ir_face_camera_metadata_history: &mut TimeSeries<(Option<ImageId>, IrLed, Duration)>,
    ) -> Self {
        ir_face_camera_metadata_history
            .iter()
            .map(|value| {
                let Timestamped { value: (image_id, wavelength, capture_timestamp), timestamp } =
                    value;
                IrFaceCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                    ),
                    wavelength: *wavelength,
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(Option<ImageId>, rgb_net::EstimateOutput, Duration)>>
    for Vec<RgbCameraMetadata>
{
    fn from(
        rgb_net_metadata_history: &mut TimeSeries<(
            Option<ImageId>,
            rgb_net::EstimateOutput,
            Duration,
        )>,
    ) -> Self {
        rgb_net_metadata_history
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, rgb_net_metadata, capture_timestamp),
                    timestamp,
                } = value;
                RgbCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                    ),
                    rgbnet: Some(rgb_net_metadata.clone()),
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(Option<ImageId>, FaceIdentifierIsValidMetadata, Duration)>>
    for Vec<SelfCustodyRgbCameraMetadata>
{
    fn from(
        face_identifier_metadata: &mut TimeSeries<(
            Option<ImageId>,
            FaceIdentifierIsValidMetadata,
            Duration,
        )>,
    ) -> Self {
        face_identifier_metadata
            .iter()
            .map(|value| {
                let Timestamped {
                    value: (image_id, face_identifier_metadata, capture_timestamp),
                    timestamp,
                } = value;
                SelfCustodyRgbCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                    ),
                    face_identifier_metadata: face_identifier_metadata.clone(),
                }
            })
            .collect()
    }
}

// TODO: Consider implementing the Serialize trait for TimeSeries<T> instead
impl From<&'_ mut TimeSeries<(Option<ImageId>, IrLed, Duration)>> for Vec<ThermalCameraMetadata> {
    fn from(
        thermal_camera_metadata_history: &mut TimeSeries<(Option<ImageId>, IrLed, Duration)>,
    ) -> Self {
        thermal_camera_metadata_history
            .iter()
            .map(|value| {
                let Timestamped { value: (image_id, wavelength, capture_timestamp), timestamp } =
                    value;
                ThermalCameraMetadata {
                    common_camera_metadata: CommonImageMetadata::new(
                        image_id,
                        *timestamp,
                        *capture_timestamp,
                    ),
                    wavelength: *wavelength,
                }
            })
            .collect()
    }
}

impl CommonImageMetadata {
    fn new(image_id: &Option<ImageId>, timestamp: Instant, capture_timestamp: Duration) -> Self {
        Self {
            image_id: image_id.as_ref().map(ToString::to_string),
            timestamp: Some(SerializableInstant::new(timestamp)),
            image_saved: image_id.is_some(),
            optical: Default::default(),
            roi: Default::default(),
            execution_timestamp: Default::default(),
            capture_timestamp,
        }
    }
}

////////////////////////////// SignupMetadata //////////////////////////////

#[derive(Serialize, JsonSchema)]
pub struct DebugMetadata {
    start_timestamp: f64,
    end_timestamp: f64,
    biometric_capture_succeeded: bool,
    signup_status: SingupStatus,
    hardware: HardwareVersion,
    optics: OpticsVersion,
    software_version: SoftwareVersion,
    orb: OrbMetadata,
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
}

#[derive(Serialize, JsonSchema, Default)]
pub struct FraudCheckResults {
    pub report: Option<fraud_check::Report>,
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
    overall_software_version: String,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct OrbMetadata {
    orb_id: String,
    distributor_id: String,
    data_policy: String,
    backend_environment: Option<String>,
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
    pub self_custody_thumbnail: Option<ImageId>,
    pub left_iris_normalized_image: Option<ImageId>,
    pub left_iris_normalized_mask: Option<ImageId>,
    pub right_iris_normalized_image: Option<ImageId>,
    pub right_iris_normalized_mask: Option<ImageId>,
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

#[derive(Serialize, JsonSchema, Default, Debug)]
struct HardwareComponentConfig {
    ir_camera: IrCameraConfig,
    rgb_camera: RgbCameraConfig,
    front_ir_camera: FrontIrCameraConfig,
    heat_camera: HeatCameraConfig,
    tof2d: Tof2dConfig,
    ir_led: IrLedConfig,
    led: WhiteLedConfig,
    mirror: MirrorConfig,
    voice: VoiceConfig,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct IrCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    liquid_lens: Vec<Timestamped<Option<i16>>>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct RgbCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    shutter: Vec<()>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct FrontIrCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct HeatCameraConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct Tof2dConfig {
    #[serde(flatten)]
    common_config: CommonCameraConfig,
    mode: Vec<()>,
    distance_filter: Vec<()>,
    filter_level: Vec<()>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct CommonCameraConfig {
    auto_exposure: Vec<Timestamped<i64>>,
    auto_gain: Vec<Timestamped<i64>>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct IrLedConfig {
    is_on: Vec<bool>,
    wavelength: Vec<Timestamped<IrLed>>,
    duration: Vec<Timestamped<u16>>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct WhiteLedConfig {
    brightness: Vec<Timestamped<u8>>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct MirrorConfig {
    left_horizontal: Vec<Timestamped<f64>>,
    left_vertical: Vec<Timestamped<f64>>,
    right_horizontal: Vec<Timestamped<f64>>,
    right_vertical: Vec<Timestamped<f64>>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct VoiceConfig {
    voice: Vec<String>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct ShutterConfig {
    side: Vec<Timestamped<bool>>,
}

////////////////////////////// RgbCameraMetadata //////////////////////////////

#[derive(Serialize, JsonSchema, Default, Debug)]
struct RgbCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    rgbnet: Option<rgb_net::EstimateOutput>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
pub struct CommonImageMetadata {
    image_id: Option<String>,
    timestamp: Option<SerializableInstant>,
    image_saved: bool,
    optical: CommonOpticalMetadata,
    roi: CommonCameraRoi,
    execution_timestamp: CommonCameraExecutionTimestamp,
    capture_timestamp: Duration,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct CommonOpticalMetadata {
    gain: Option<i64>,
    exposure: Option<u8>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct Coordinate {
    x: Option<u16>,
    y: Option<u16>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct CommonCameraRoi {
    top_left_coordinate: Coordinate,
    bottom_right_coordinate: Coordinate,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
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

#[derive(Serialize, JsonSchema, Default, Debug, Clone)]
struct RgbNetEyeCoordinates {
    left: (f64, f64),
    right: (f64, f64),
    left_undistorted: (f64, f64),
    right_undistorted: (f64, f64),
}

#[allow(clippy::struct_field_names)]
#[derive(Serialize, JsonSchema, Default, Debug, Clone)]
struct RgbNetFaceCoordinates {
    distorted_start_x: f64,
    distorted_start_y: f64,
    distorted_end_x: f64,
    distorted_end_y: f64,
}

////////////////////////////// IrCameraMetadata //////////////////////////////

#[derive(Serialize, JsonSchema)]
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

#[derive(Serialize, JsonSchema)]
pub struct IrFaceCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    /// Wavelength of the ir LEDs at the time of capture
    wavelength: IrLed,
}

#[derive(Serialize, JsonSchema)]
pub struct SelfCustodyRgbCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    face_identifier_metadata: FaceIdentifierIsValidMetadata,
}

////////////////////////////// HeatCameraMetadata //////////////////////////////

#[derive(Serialize, JsonSchema)]
pub struct ThermalCameraMetadata {
    #[serde(flatten)]
    common_camera_metadata: CommonImageMetadata,
    /// Wavelength of the ir LEDs at the time of capture.
    wavelength: IrLed,
}

////////////////////////////// InternalStateData //////////////////////////////

#[derive(Serialize, JsonSchema, Default, Debug)]
struct InternalStateData {
    autofocus: InternalStateAutofocus,
    mirror_eye_tracking_pid: MirrorEyeTrackingPid,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct InternalStateAutofocus {
    crit_sharpness: f64,
    valid_history_delay: f64,
    latency_focus_setting: f64,
    left: AutofocusTarget,
    right: AutofocusTarget,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct AutofocusTarget {
    target: ValueOffsetTimestamp,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct ValueOffsetTimestamp {
    value: f64,
    offset: f64,
    timestamp: f64,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct MirrorEyeTrackingPid {
    left: Vec<MirrorOffsetSetting>,
    right: Vec<MirrorOffsetSetting>,
}

#[derive(Serialize, JsonSchema, Default, Debug)]
struct MirrorOffsetSetting {
    x: f64,
    y: f64,
    timestamp: f64,
}
