//! Orb configuration settings.

use crate::{
    agents::python::face_identifier,
    backend,
    consts::{
        CONFIG_DIR, DEFAULT_BLOCK_SIGNUPS_WHEN_NO_INTERNET, DEFAULT_MAX_FAN_SPEED,
        DEFAULT_SLOW_INTERNET_PING_THRESHOLD, DEFAULT_SOUND_VOLUME,
        DEFAULT_THERMAL_CAMERA_PAIRING_STATUS_TIMEOUT, MAX_SOUND_VOLUME,
    },
    logger::{LogOnError, DATADOG, NO_TAGS},
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
#[derive(Clone, Serialize, JsonSchema)]
#[cfg_attr(feature = "stage", derive(Debug))]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    /// This part of the config is safe to write to the disk.
    pub basic_config: BasicConfig,
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
    /// Fraud check engine: config collection.
    pub fraud_check_engine_config: fraud_check::BackendConfig,
    /// IR-Net model configs: Namespaced IR-Net configs.
    pub ir_net_model_configs: Option<HashMap<String, String>>,
    /// Iris model configs: Namespaced Iris config files.
    pub iris_model_configs: Option<HashMap<String, String>>,
    /// Face Identifier: Namespaced Face Identifier configs collection.
    pub face_identifier_model_configs: face_identifier::types::BackendConfig,
    /// How long the thermal camera agent will wait until it assumes
    /// the cam is stuck pairing.
    pub thermal_camera_pairing_status_timeout: Duration,
    /// Whether the thermal camera agent is enabled or not.
    pub thermal_camera: bool,
    /// Upload self-custody images to backend.
    pub upload_self_custody_images: bool,
    /// Upload self-custody thumbnail to backend.
    pub upload_self_custody_thumbnail: bool,
    /// Upload Iris' normalized images to backend.
    pub upload_iris_normalized_images: bool,
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
                    fan_max_speed,
                    slow_internet_ping_threshold,
                    block_signup_when_no_internet,
                    ir_eye_save_fps_override,
                    ir_face_save_fps_override,
                    thermal_save_fps_override,
                    fraud_check_engine_config,
                    ir_net_model_configs,
                    iris_model_configs,
                    face_identifier_model_configs,
                    thermal_camera_pairing_status_timeout,
                    thermal_camera,
                    upload_self_custody_images,
                    upload_self_custody_thumbnail,
                    upload_iris_normalized_images,
                    last_updated: _,
                },
        } = status;
        Some(Self {
            basic_config: BasicConfig {
                sound_volume: sound_volume.clamp(0, MAX_SOUND_VOLUME),
                language,
            },
            fan_max_speed: Some(
                fan_max_speed.unwrap_or(DEFAULT_MAX_FAN_SPEED).clamp(0.0, DEFAULT_MAX_FAN_SPEED),
            ),
            slow_internet_ping_threshold: slow_internet_ping_threshold
                .map_or(Self::default().slow_internet_ping_threshold, Duration::from_millis),
            block_signup_when_no_internet,
            ir_eye_save_fps_override,
            ir_face_save_fps_override,
            thermal_save_fps_override,
            fraud_check_engine_config,
            ir_net_model_configs,
            iris_model_configs,
            face_identifier_model_configs,
            thermal_camera_pairing_status_timeout: thermal_camera_pairing_status_timeout.map_or(
                Self::default().thermal_camera_pairing_status_timeout,
                Duration::from_millis,
            ),
            thermal_camera: thermal_camera.unwrap_or(Self::default().thermal_camera),
            upload_self_custody_images: upload_self_custody_images
                .unwrap_or(Self::default().upload_self_custody_images),
            upload_self_custody_thumbnail: upload_self_custody_thumbnail
                .unwrap_or(Self::default().upload_self_custody_thumbnail),
            upload_iris_normalized_images: upload_iris_normalized_images
                .unwrap_or(Self::default().upload_iris_normalized_images),
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
            DATADOG.incr("orb.main.count.http.config_update.error", NO_TAGS).or_log();
            e
        })?;

        if let Some(config) = Config::from_backend(res) {
            DATADOG.incr("orb.main.count.http.config_update.success", NO_TAGS)?;
            Ok(config)
        } else {
            DATADOG.incr("orb.main.count.http.config_parse.error", NO_TAGS).or_log();
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
            fan_max_speed: Some(DEFAULT_MAX_FAN_SPEED),
            slow_internet_ping_threshold: DEFAULT_SLOW_INTERNET_PING_THRESHOLD,
            block_signup_when_no_internet: DEFAULT_BLOCK_SIGNUPS_WHEN_NO_INTERNET,
            ir_eye_save_fps_override: None,
            ir_face_save_fps_override: None,
            thermal_save_fps_override: None,
            fraud_check_engine_config: fraud_check::BackendConfig {},
            ir_net_model_configs: None,
            iris_model_configs: None,
            face_identifier_model_configs: face_identifier::types::BackendConfig {},
            thermal_camera_pairing_status_timeout: DEFAULT_THERMAL_CAMERA_PAIRING_STATUS_TIMEOUT,
            thermal_camera: false,
            upload_self_custody_images: false,
            upload_self_custody_thumbnail: true,
            upload_iris_normalized_images: true,
        }
    }
}

fn config_file_path() -> PathBuf {
    Path::new(CONFIG_DIR).join("config.json")
}
