//! Orb configuration settings.

use crate::{
    agents::python::face_identifier,
    backend,
    consts::{
        CONFIG_DIR, DEFAULT_BIOMETRIC_CAPTURE_TIMEOUT_SELF_SERVE,
        DEFAULT_BLOCK_SIGNUPS_WHEN_NO_INTERNET, DEFAULT_MAX_FAN_SPEED,
        DEFAULT_SLOW_INTERNET_PING_THRESHOLD, DEFAULT_SOUND_VOLUME,
        DEFAULT_THERMAL_CAMERA_PAIRING_STATUS_TIMEOUT, MAX_SOUND_VOLUME, QR_SCAN_TIMEOUT,
    },
    dd_incr, identification,
    plans::fraud_check,
};
use eyre::{eyre, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, sync::Mutex};

/// Configuration settings that are safe to write to the disk. We still need to write part of the configuration to the
/// disk as initially the orb might not have internet connection (e.g. on first boot in a new area) so a default or last
/// set language and volume is needed. All other configuration settings are downloaded from the backend and internet
/// connection is mandatory for the orb to function.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Serialize, Deserialize, JsonSchema, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct BasicConfig {
    /// The sound volume configuration.
    pub sound_volume: u64,
    /// UI language. If not set, US English is assumed.
    pub language: Option<String>,
}

/// Orb configuration settings.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "stage", derive(Debug))]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    /// This part of the config is safe to write to the disk.
    #[serde(flatten)]
    pub basic_config: BasicConfig,
    /// The country the Orb is expected to operate for signups.
    pub operation_country: Option<String>,
    /// The city the Orb is expected to operate for signups.
    pub operation_city: Option<String>,
    /// The fan max speed.
    pub fan_max_speed: Option<f32>,
    /// Threshold for configuring what's the maximum ping delay for the internet connection, to warn for delays in the
    /// signup process.
    pub slow_internet_ping_threshold: Duration,
    /// If `true`, signups are blocked when no internet connection is detected.
    #[serde(default)]
    pub block_signup_when_no_internet: bool,
    /// Override the FPS for saving IR eye frames.
    pub ir_eye_save_fps_override: Option<f32>,
    /// Override the FPS for saving IR face frames.
    pub ir_face_save_fps_override: Option<f32>,
    /// Override the FPS for saving thermal frames.
    pub thermal_save_fps_override: Option<f32>,
    /// Contact lens model config.
    pub contact_lens_model_config: Option<String>,
    /// Fraud check engine: config collection.
    pub fraud_check_engine_config: fraud_check::BackendConfig,
    /// IR-Net model configs: Namespaced IR-Net configs.
    pub ir_net_model_configs: Option<HashMap<String, String>>,
    /// Iris model configs: Namespaced Iris config files.
    pub iris_model_configs: Option<HashMap<String, String>>,
    /// Person Classifier config: under-age threshold.
    pub child_threshold: Option<f32>,
    /// Face Identifier: Namespaced Face Identifier configs collection.
    pub face_identifier_model_configs: face_identifier::types::BackendConfig,
    /// How long the thermal camera agent will wait until it assumes
    /// the cam is stuck pairing.
    pub thermal_camera_pairing_status_timeout: Duration,
    /// Whether the thermal camera agent is enabled or not.
    pub thermal_camera: bool,
    /// Whether the depth camera agent is enabled or not.
    pub depth_camera: bool,
    /// Self-serve mode.
    pub self_serve: bool,
    /// Alternative mode for self-serve: start a signup with a button press.
    pub self_serve_button: bool,
    /// Ask the operator for a QR code when a possibly underaged person is detected.
    pub self_serve_ask_op_qr_for_possibly_underaged: bool,
    /// How long to wait for the operator to scan the QR code when a possibly underaged person is detected.
    pub self_serve_ask_op_qr_for_possibly_underaged_timeout: Duration,
    /// If `true`, we don't wait for the user to use the mobile app to start the biometric capture, we instantly start.
    pub self_serve_app_skip_capture_trigger: bool,
    /// How long to wait for the user to start the biometric capture from the app in self-serve mode.
    pub self_serve_app_capture_trigger_timeout: Duration,
    /// Biometric capture time-out in self-serve mode.
    pub self_serve_biometric_capture_timeout: Duration,
    /// Default phi offset for the mirror if no calibration.json is present.
    pub mirror_default_phi_offset_degrees: f64,
    /// Default theta offset for the mirror if no calibration.json is present.
    pub mirror_default_theta_offset_degrees: f64,
    /// Pruning log messages from process agents.
    pub process_agent_logger_pruning: bool,
    /// HTTP client to backend: request timeout.
    pub backend_http_request_timeout: Duration,
    /// HTTP client to backend: connect timeout.
    pub backend_http_connect_timeout: Duration,
    /// Personal Custody Package v3 feature flag.
    pub pcp_v3: bool,
    /// Number of PCP Tier 1 packages in the queue when signups are blocked.
    pub pcp_tier1_blocking_threshold: u32,
    /// Maximum number of PCP Tier 1 packages in the queue before we start dropping them.
    pub pcp_tier1_dropping_threshold: u32,
    /// Number of PCP Tier 2 packages in the queue when signups are blocked.
    pub pcp_tier2_blocking_threshold: u32,
    /// Maximum number of PCP Tier 2 packages in the queue before we start dropping them.
    pub pcp_tier2_dropping_threshold: u32,
    /// Ignore app centric signup flag from the app and always perform an enrollment request.
    pub ignore_user_centric_signups: bool,
    /// Use the operator's QR together with the user QR to validate the user.
    pub user_qr_validation_use_full_operator_qr: bool,
    /// Use only the operator's location to validate the user.
    pub user_qr_validation_use_only_operator_location: bool,
    /// Orb relay: wait for pending messages before shutting down.
    pub orb_relay_shutdown_wait_for_pending_messages: Duration,
    /// Orb relay: wait for the agent to shutdown after sending shutdown message from the client.
    pub orb_relay_shutdown_wait_for_shutdown: Duration,
    /// Orb relay: number of retries to announce the orb id.
    pub orb_relay_announce_orb_id_retries: u32,
    /// Orb relay: timeout for waiting announce_orb_id to be ack from the server.
    pub orb_relay_announce_orb_id_timeout: Duration,
    /// Expiration time for the operator QR code.
    pub operator_qr_expiration_time: Duration,
}

#[cfg(not(feature = "stage"))]
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "*Redacted in prod: get the config from debug-report. Use 'stage' for printing*")
    }
}

impl Config {
    /// Creates a new configuration object from the orb status response.
    ///
    /// Returns:
    /// `Config` on successful validation, `None` otherwise.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn from_backend(status: backend::config::Response) -> Option<Self> {
        let backend::config::Response {
            config:
                backend::config::Config {
                    sound_volume,
                    language,
                    operation_country,
                    operation_city,
                    fan_max_speed,
                    slow_internet_ping_threshold,
                    block_signup_when_no_internet,
                    ir_eye_save_fps_override,
                    ir_face_save_fps_override,
                    thermal_save_fps_override,
                    contact_lens_model_config,
                    fraud_check_engine_config,
                    ir_net_model_configs,
                    iris_model_configs,
                    child_threshold,
                    face_identifier_model_configs,
                    thermal_camera_pairing_status_timeout,
                    thermal_camera,
                    depth_camera,
                    self_serve,
                    self_serve_button,
                    self_serve_ask_op_qr_for_possibly_underaged,
                    self_serve_ask_op_qr_for_possibly_underaged_timeout,
                    self_serve_app_skip_capture_trigger,
                    self_serve_app_capture_trigger_timeout,
                    self_serve_biometric_capture_timeout,
                    mirror_default_phi_offset_degrees,
                    mirror_default_theta_offset_degrees,
                    process_agent_logger_pruning,
                    backend_http_request_timeout,
                    backend_http_connect_timeout,
                    pcp_v3,
                    pcp_tier1_blocking_threshold,
                    pcp_tier1_dropping_threshold,
                    pcp_tier2_blocking_threshold,
                    pcp_tier2_dropping_threshold,
                    ignore_user_centric_signups,
                    user_qr_validation_use_full_operator_qr,
                    user_qr_validation_use_only_operator_location,
                    orb_relay_shutdown_wait_for_pending_messages,
                    orb_relay_shutdown_wait_for_shutdown,
                    orb_relay_announce_orb_id_retries,
                    orb_relay_announce_orb_id_timeout,
                    operator_qr_expiration_time,
                    last_updated: _,
                },
        } = status;
        let default = Self::default();
        Some(Self {
            basic_config: BasicConfig {
                sound_volume: sound_volume.clamp(0, MAX_SOUND_VOLUME),
                language,
            },
            operation_country: operation_country.or(default.operation_country),
            operation_city: operation_city.or(default.operation_city),
            fan_max_speed: Some(
                fan_max_speed.unwrap_or(DEFAULT_MAX_FAN_SPEED).clamp(0.0, DEFAULT_MAX_FAN_SPEED),
            ),
            slow_internet_ping_threshold: slow_internet_ping_threshold
                .map_or(default.slow_internet_ping_threshold, Duration::from_millis),
            block_signup_when_no_internet,
            ir_eye_save_fps_override,
            ir_face_save_fps_override,
            thermal_save_fps_override,
            contact_lens_model_config,
            fraud_check_engine_config,
            ir_net_model_configs,
            iris_model_configs,
            child_threshold,
            face_identifier_model_configs,
            thermal_camera_pairing_status_timeout: thermal_camera_pairing_status_timeout
                .map_or(default.thermal_camera_pairing_status_timeout, Duration::from_millis),
            thermal_camera: thermal_camera.unwrap_or(default.thermal_camera),
            depth_camera: depth_camera.unwrap_or(default.depth_camera),
            self_serve: self_serve.unwrap_or(default.self_serve),
            self_serve_button: self_serve_button.unwrap_or(default.self_serve_button),
            self_serve_ask_op_qr_for_possibly_underaged:
                self_serve_ask_op_qr_for_possibly_underaged
                    .unwrap_or(default.self_serve_ask_op_qr_for_possibly_underaged),
            self_serve_ask_op_qr_for_possibly_underaged_timeout:
                self_serve_ask_op_qr_for_possibly_underaged_timeout.map_or(
                    default.self_serve_ask_op_qr_for_possibly_underaged_timeout,
                    Duration::from_millis,
                ),
            self_serve_app_skip_capture_trigger: self_serve_app_skip_capture_trigger
                .unwrap_or(default.self_serve_app_skip_capture_trigger),
            self_serve_app_capture_trigger_timeout: self_serve_app_capture_trigger_timeout
                .map_or(default.self_serve_app_capture_trigger_timeout, Duration::from_millis),
            self_serve_biometric_capture_timeout: self_serve_biometric_capture_timeout
                .map_or(default.self_serve_biometric_capture_timeout, Duration::from_millis),
            mirror_default_phi_offset_degrees: mirror_default_phi_offset_degrees
                .unwrap_or(default.mirror_default_phi_offset_degrees),
            mirror_default_theta_offset_degrees: mirror_default_theta_offset_degrees
                .unwrap_or(default.mirror_default_theta_offset_degrees),
            process_agent_logger_pruning: process_agent_logger_pruning
                .unwrap_or(default.process_agent_logger_pruning),
            backend_http_request_timeout: backend_http_request_timeout
                .map_or(default.backend_http_request_timeout, Duration::from_millis),
            backend_http_connect_timeout: backend_http_connect_timeout
                .map_or(default.backend_http_connect_timeout, Duration::from_millis),
            pcp_v3: pcp_v3.unwrap_or(default.pcp_v3),
            pcp_tier1_blocking_threshold: pcp_tier1_blocking_threshold
                .unwrap_or(default.pcp_tier1_blocking_threshold),
            pcp_tier1_dropping_threshold: pcp_tier1_dropping_threshold
                .unwrap_or(default.pcp_tier1_dropping_threshold),
            pcp_tier2_blocking_threshold: pcp_tier2_blocking_threshold
                .unwrap_or(default.pcp_tier2_blocking_threshold),
            pcp_tier2_dropping_threshold: pcp_tier2_dropping_threshold
                .unwrap_or(default.pcp_tier2_dropping_threshold),
            ignore_user_centric_signups: ignore_user_centric_signups
                .unwrap_or(default.ignore_user_centric_signups),
            user_qr_validation_use_full_operator_qr: user_qr_validation_use_full_operator_qr
                .unwrap_or(default.user_qr_validation_use_full_operator_qr),
            user_qr_validation_use_only_operator_location:
                user_qr_validation_use_only_operator_location
                    .unwrap_or(default.user_qr_validation_use_only_operator_location),
            orb_relay_shutdown_wait_for_pending_messages:
                orb_relay_shutdown_wait_for_pending_messages.map_or(
                    default.orb_relay_shutdown_wait_for_pending_messages,
                    Duration::from_millis,
                ),
            orb_relay_shutdown_wait_for_shutdown: orb_relay_shutdown_wait_for_shutdown
                .map_or(default.orb_relay_shutdown_wait_for_shutdown, Duration::from_millis),
            orb_relay_announce_orb_id_retries: orb_relay_announce_orb_id_retries
                .unwrap_or(default.orb_relay_announce_orb_id_retries),
            orb_relay_announce_orb_id_timeout: orb_relay_announce_orb_id_timeout
                .map_or(default.orb_relay_announce_orb_id_timeout, Duration::from_millis),
            operator_qr_expiration_time: operator_qr_expiration_time
                .map_or(default.operator_qr_expiration_time, Duration::from_millis),
        })
        .filter(Self::validate)
    }

    /// Tries to load config from the file system, or constructs a default
    /// config on failure.
    pub async fn load_or_default() -> Self {
        Self::load()
            .await
            .map_err(|err| {
                tracing::error!(
                    "Cached config loading error. Will continue with providing a default one: \
                     {err:#?}"
                );
            })
            .ok()
            .filter(Self::validate)
            .unwrap_or_default()
    }

    /// Send freshly loaded config to UI engine
    pub fn propagate_to_ui(&self, ui: &dyn crate::ui::Engine) {
        ui.sound_volume(self.sound_volume());
        ui.sound_language(self.language().clone());
    }

    /// Stores the configuration settings to the file system.
    pub async fn store(&self) -> Result<()> {
        fs::create_dir_all(CONFIG_DIR).await?;
        let json = serde_json::to_string_pretty(&self.basic_config)
            .wrap_err("config storing failed due to unserializable format with serde_json")?;
        tracing::info!("Storing configuration settings: {}", json);
        let path = config_file_path();
        fs::write(path, json).await?;
        Ok(())
    }

    /// Validates the configuration.
    #[must_use]
    pub fn validate(&self) -> bool {
        self.basic_config.sound_volume <= MAX_SOUND_VOLUME
    }

    async fn load() -> Result<Self> {
        let path = config_file_path();
        tracing::info!("Loading config from {}", path.display());
        let contents = fs::read_to_string(path).await?;
        tracing::debug!("Config file contents: {contents:#?}");
        Ok(Self { basic_config: serde_json::from_str(&contents)?, ..Self::default() })
    }

    /// Downloads the latest configuration from the backend and updates the
    /// shared configuration object.
    pub async fn download() -> Result<Config> {
        let res = backend::config::request().await.map_err(|e| {
            tracing::error!("Config request failed: {:?}", e);
            dd_incr!("main.count.http.config_update.error");
            e
        })?;

        if let Some(config) = Config::from_backend(res) {
            dd_incr!("main.count.http.config_update.success");
            Ok(config)
        } else {
            dd_incr!("main.count.http.config_parse.error");
            Err(eyre!("invalid config"))
        }
    }

    /// Downloads the latest configuration from the backend, updates the shared
    /// configuration object, and stores the updated configuration to the file
    /// system.
    pub async fn download_and_store(config: Arc<Mutex<Config>>) -> Result<()> {
        *config.lock().await = Self::download().await.map_err(|e| {
            tracing::error!("Failed to download config: {:?}", e);
            e
        })?;
        let config_to_store = config.lock().await;
        tracing::info!("Downloaded latest config: {:?}", config_to_store);
        config_to_store.store().await.map_err(|e| {
            tracing::error!("Config downloaded but failed to be stored: {:?}", e);
            e
        })
    }

    /// Returns the sound volume.
    #[must_use]
    pub fn sound_volume(&self) -> u64 {
        self.basic_config.sound_volume
    }

    /// Returns the language.
    #[must_use]
    pub fn language(&self) -> &Option<String> {
        &self.basic_config.language
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            basic_config: BasicConfig { sound_volume: DEFAULT_SOUND_VOLUME, language: None },
            operation_country: if cfg!(feature = "stage") { Some("DEV".to_owned()) } else { None },
            operation_city: if cfg!(feature = "stage") { Some("DEV".to_owned()) } else { None },
            fan_max_speed: Some(DEFAULT_MAX_FAN_SPEED),
            slow_internet_ping_threshold: DEFAULT_SLOW_INTERNET_PING_THRESHOLD,
            block_signup_when_no_internet: DEFAULT_BLOCK_SIGNUPS_WHEN_NO_INTERNET,
            ir_eye_save_fps_override: None,
            ir_face_save_fps_override: None,
            thermal_save_fps_override: None,
            contact_lens_model_config: None,
            fraud_check_engine_config: fraud_check::BackendConfig {},
            ir_net_model_configs: None,
            iris_model_configs: None,
            child_threshold: None,
            face_identifier_model_configs: face_identifier::types::BackendConfig {
                face_identifier_model_configs: None,
            },
            thermal_camera_pairing_status_timeout: DEFAULT_THERMAL_CAMERA_PAIRING_STATUS_TIMEOUT,
            thermal_camera: false,
            depth_camera: false,
            self_serve: false,
            self_serve_button: false,
            self_serve_ask_op_qr_for_possibly_underaged: false,
            self_serve_ask_op_qr_for_possibly_underaged_timeout: QR_SCAN_TIMEOUT,
            self_serve_app_skip_capture_trigger: false,
            // TODO: This is for demo purposes, we should reduce this eventually when the video comes before the QR.
            self_serve_app_capture_trigger_timeout: Duration::from_millis(120_000),
            self_serve_biometric_capture_timeout: DEFAULT_BIOMETRIC_CAPTURE_TIMEOUT_SELF_SERVE,
            mirror_default_phi_offset_degrees: if identification::HARDWARE_VERSION
                .contains("Diamond")
            {
                0.0
            } else {
                -0.46
            },
            mirror_default_theta_offset_degrees: if identification::HARDWARE_VERSION
                .contains("Diamond")
            {
                0.0
            } else {
                -0.35
            },
            process_agent_logger_pruning: !cfg!(feature = "stage"),
            backend_http_request_timeout: Duration::from_millis(60_000 * 3),
            backend_http_connect_timeout: Duration::from_millis(30_000),
            pcp_v3: false,
            pcp_tier1_blocking_threshold: 12,
            pcp_tier1_dropping_threshold: u32::MAX,
            pcp_tier2_blocking_threshold: u32::MAX,
            pcp_tier2_dropping_threshold: 12,
            ignore_user_centric_signups: false,
            user_qr_validation_use_full_operator_qr: false,
            user_qr_validation_use_only_operator_location: true,
            orb_relay_shutdown_wait_for_pending_messages: Duration::from_millis(1500),
            orb_relay_shutdown_wait_for_shutdown: Duration::from_millis(1500),
            orb_relay_announce_orb_id_retries: 3,
            orb_relay_announce_orb_id_timeout: Duration::from_millis(2000),
            operator_qr_expiration_time: Duration::from_secs(60 * 60 * 23),
        }
    }
}

fn config_file_path() -> PathBuf {
    Path::new(CONFIG_DIR).join("config.json")
}
