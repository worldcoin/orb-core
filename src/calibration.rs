//! Calibration data.
use crate::{config::Config, dd_gauge};
use eyre::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

const VERSION: &str = "v2";

/// Calibration data.
#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Calibration {
    pub mirror: Mirror,
}

#[allow(missing_docs)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mirror {
    pub phi_offset_degrees: f64,
    pub theta_offset_degrees: f64,
    pub version: String,
}

impl Calibration {
    /// Stores the calibration data to the file system.
    pub async fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        dd_gauge!(
            "main.gauge.system.calibration.mirror",
            self.mirror.phi_offset_degrees.to_string(),
            "type:phi_degrees"
        );
        dd_gauge!(
            "main.gauge.system.calibration.mirror",
            self.mirror.theta_offset_degrees.to_string(),
            "type:theta_degrees"
        );
        tracing::info!("Storing calibration data: {}", json);
        fs::write(path, json).await.map_err(Into::into)
    }

    /// Tries to load calibration from the file system.
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            tracing::error!("Calibration file: {} not exists", path.display());
            bail!("Calibration file not exists");
        }
        tracing::info!("Loading calibration file: {}", path.display());
        let contents = fs::read_to_string(path).await?;
        tracing::debug!("Calibration file contents: {contents:#?}");

        match serde_json::from_str::<Calibration>(&contents) {
            Ok(config) if config.mirror.version == VERSION => Ok(config),
            Ok(config) => {
                tracing::warn!(
                    "Calibration file version: {}, will update to: {VERSION}",
                    config.mirror.version
                );
                bail!("Calibration file version mismatch");
            }
            Err(e) => {
                tracing::error!("Calibration loading error: {e:#?}");
                Err(e.into())
            }
        }
    }
}

impl From<&Config> for Calibration {
    fn from(config: &Config) -> Self {
        Calibration {
            mirror: Mirror {
                phi_offset_degrees: config.mirror_default_phi_offset_degrees,
                theta_offset_degrees: config.mirror_default_theta_offset_degrees,
                version: VERSION.to_owned(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile;
    use tokio::sync::Mutex;

    #[allow(clippy::float_cmp)]
    #[tokio::test]
    async fn test_default_creates_expected_calibration() {
        let config = Arc::new(Mutex::new(Config {
            mirror_default_phi_offset_degrees: 1.0,
            mirror_default_theta_offset_degrees: 2.0,
            ..Default::default()
        }));

        let calibration: Calibration = (&*config.lock().await).into();
        assert_eq!(calibration.mirror.phi_offset_degrees, 1.0);
        assert_eq!(calibration.mirror.theta_offset_degrees, 2.0);
        assert_eq!(calibration.mirror.version, "v2");
    }

    #[allow(clippy::float_cmp)]
    #[tokio::test]
    async fn test_store_and_load_calibration() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let config_path = temp_dir.path().join("calibration.json");

        let calibration = Calibration {
            mirror: Mirror {
                phi_offset_degrees: 3.0,
                theta_offset_degrees: 4.0,
                version: "v2".to_owned(),
            },
        };

        calibration.store(&config_path).await.expect("to store calibration");

        let loaded_calibration =
            Calibration::load(&config_path).await.expect("to load calibration");
        assert_eq!(loaded_calibration.mirror.phi_offset_degrees, 3.0);
        assert_eq!(loaded_calibration.mirror.theta_offset_degrees, 4.0);
        assert_eq!(loaded_calibration.mirror.version, "v2".to_owned());
    }

    #[allow(clippy::float_cmp)]
    #[tokio::test]
    async fn test_bad_load_or_default() {
        let temp_dir = tempfile::tempdir().expect("to create temp dir");
        let config_path = temp_dir.path().join("calibration.json");

        let config = Arc::new(Mutex::new(Config {
            mirror_default_phi_offset_degrees: 1.0,
            mirror_default_theta_offset_degrees: 2.0,
            ..Default::default()
        }));

        // Unsupported version
        let json =
            r#"{"mirror":{"phi_offset_degrees":3.0,"theta_offset_degrees":4.0,"version":"v3"}}"#;
        fs::write(&config_path, json).await.expect("to write calibration.json");

        let calibration: Calibration = if Calibration::load(config_path.clone()).await.is_ok() {
            unreachable!("Should not be able to load calibration")
        } else {
            (&*config.lock().await).into()
        };
        assert_eq!(calibration.mirror.phi_offset_degrees, 1.0);
        assert_eq!(calibration.mirror.theta_offset_degrees, 2.0);
        assert_eq!(calibration.mirror.version, "v2");

        // Old format
        let json =
            r#"{"mirror":{"phi_neutral_angle_degrees":3.0,"theta_neutral_angle_degrees":4.0}}"#;
        fs::write(&config_path, json).await.expect("to write calibration.json");

        let calibration: Calibration = if Calibration::load(config_path.clone()).await.is_ok() {
            unreachable!("Should not be able to load calibration")
        } else {
            (&*config.lock().await).into()
        };
        assert_eq!(calibration.mirror.phi_offset_degrees, 1.0);
        assert_eq!(calibration.mirror.theta_offset_degrees, 2.0);
        assert_eq!(calibration.mirror.version, "v2");
    }
}
