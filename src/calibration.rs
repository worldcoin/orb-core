//! Calibration data.
use crate::{consts::CONFIG_DIR, logger::DATADOG};
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Calibration data.
#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Calibration {
    pub mirror: Mirror,
}

#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mirror {
    // TODO: remove after transition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizontal_neutral_angle: Option<f64>,
    // TODO: remove after transition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_neutral_angle: Option<f64>,
    // TODO: remove default attribute
    #[serde(default)]
    pub horizontal_offset: f64,
    // TODO: remove default attribute
    #[serde(default)]
    pub vertical_offset: f64,
}

impl Calibration {
    /// Tries to load calibration from the file system, or constructs a default
    /// config on failure.
    pub async fn load_or_default() -> Self {
        Self::load()
            .await
            .map_err(|err| tracing::error!("Calibration loading error: {err:#?}"))
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Stores the calibration data to the file system.
    pub async fn store(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        DATADOG.gauge(
            "orb.main.gauge.system.calibration.mirror",
            self.mirror.horizontal_offset.to_string(),
            ["type:horizontal"],
        )?;
        DATADOG.gauge(
            "orb.main.gauge.system.calibration.mirror",
            self.mirror.vertical_offset.to_string(),
            ["type:vertical"],
        )?;
        tracing::info!("Storing calibration data: {}", json);
        let path = calibration_file_path();
        fs::write(path, json).await?;
        Ok(())
    }

    async fn load() -> Result<Option<Self>> {
        let path = calibration_file_path();
        if !path.exists() {
            tracing::info!("Calibration file at {} not exists", path.display());
            return Ok(None);
        }
        tracing::info!("Loading calibration from {}", path.display());
        let contents = fs::read_to_string(path).await?;
        tracing::debug!("Calibration file contents: {contents:#?}");
        Ok(Some(serde_json::from_str(&contents)?))
    }
}

impl Default for Mirror {
    fn default() -> Self {
        Self {
            horizontal_neutral_angle: None,
            vertical_neutral_angle: None,
            horizontal_offset: -1.0,
            vertical_offset: -6.0,
        }
    }
}

fn calibration_file_path() -> PathBuf {
    Path::new(CONFIG_DIR).join("calibration.json")
}
