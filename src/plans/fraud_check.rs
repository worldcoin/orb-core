//! Fraud checks.
//!
//! ⚠️ Note: we have removed code related to fraud checks in this open source release.
//! See the README for the rationale.

use super::biometric_pipeline;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

/// Convenience wrapper struct for the Fraud Check Engine's configuration coming from the backend.
#[derive(
    Archive, Serialize, Deserialize, SerdeDeserialize, SerdeSerialize, Debug, Clone, JsonSchema,
)]
#[serde(rename_all = "PascalCase")]
pub struct BackendConfig {}

/// The results of the fraud checks.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, SerdeSerialize, JsonSchema, Clone)]
pub struct Report {}

impl Report {
    #[allow(clippy::unused_self)]
    fn fraud_checks(&self) -> [Option<bool>; 0] {
        []
    }

    /// If fraud data are missing, we assume fraud is detected.
    fn fraud_checks_strict(&self) -> [bool; 0] {
        self.fraud_checks().map(|v| v.unwrap_or(true))
    }

    fn enabled_checks_from_config(_config: &BackendConfig) -> [bool; 0] {
        []
    }

    /// Get the || result of all fraud checks, but under the Orb configuration.
    /// The end result might be different from the || of all fraud booleans as
    /// we might decide to not block a signup even if it's fraudulent.
    #[must_use]
    pub fn fraud_detected_with_config(&self, config: &BackendConfig) -> bool {
        Self::enabled_checks_from_config(config)
            .iter()
            .zip(self.fraud_checks_strict().iter())
            // Enable or disable fraud checks based on the Orb config.
            .map(|(&v1, &v2)| v1 && v2)
            // If any enabled fraud is true, then a fraud attempt is detected.
            .any(|v| v)
    }

    /// If any fraud check fails or is missing data, fraud is reported.
    #[must_use]
    pub fn fraud_detected(&self) -> bool {
        self.fraud_checks_strict().iter().any(|&v| v)
    }
}

/// Fraud checks plan.
#[derive(Debug)]
pub struct FraudChecks<'a> {
    _pipeline: &'a biometric_pipeline::Pipeline,
}

impl<'a> FraudChecks<'a> {
    /// Create a new FraudCheck.
    #[must_use]
    pub fn new(pipeline: &'a biometric_pipeline::Pipeline) -> Self {
        Self { _pipeline: pipeline }
    }

    /// Run all fraud checks.
    /// ⚠️ Note. We have removed fraud checks in the open sourced code. see README.md for more details.
    #[must_use]
    pub fn run(&mut self) -> Report {
        Report {}
    }
}
