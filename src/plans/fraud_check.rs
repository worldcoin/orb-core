//! Fraud checks.
#![allow(clippy::unused_self, clippy::unnecessary_wraps)]

use super::biometric_pipeline;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use std::{marker::PhantomData, time::Duration};

/// Number of fraud checks performed by the Fraud Check Engine.
/// FOSS: This is set to 0 because we manually deleted all fraud checks
const N_FRAUD_CHECKS: usize = 0;

/// Convenience wrapper struct for the Fraud Check Engine's configuration coming from the backend.
#[cfg_attr(test, derive(Default))]
#[derive(
    Archive, Serialize, Deserialize, SerdeDeserialize, SerdeSerialize, Debug, Clone, JsonSchema,
)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct BackendConfig {}

// Helper function to deserialize a Duration from a u64 representing milliseconds
#[allow(dead_code)]
fn deserialize_duration_from_millis<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = <u64 as SerdeDeserialize>::deserialize(deserializer)?;
    Ok(Duration::from_millis(millis))
}

/// The results of the fraud checks.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, SerdeSerialize, JsonSchema, Clone)]
pub struct Report {}

/// User feedback message types in case of failed pipeline.
/// This is not an exhaustive list of the true failure modes, the true
/// failure modes are more low level. This list doesn't include the actual
/// fraud based failure modes.
#[derive(Debug, Clone, SerdeSerialize, JsonSchema)]
pub enum PipelineFailureFeedbackMessage {
    /// Contact Lenses detected
    ContactLenses,
    /// Face occluded by eye glasses detected
    EyeGlasses,
    /// Face occluded by mask
    Mask,
    /// Generic face occlusion
    FaceOcclusion,
    /// Multiple faces during signup
    MultipleFaces,
    /// Eyes were occluded during signup
    EyesOcclusion,
    /// Head pose not straight up
    HeadPose,
    /// Underaged
    Underaged,
    /// Poor Image Quality
    LowImageQuality,
}

impl Report {
    const DATADOG_TAGS: [&'static str; N_FRAUD_CHECKS] = [];

    fn fraud_checks(&self) -> [Option<bool>; N_FRAUD_CHECKS] {
        []
    }

    /// If fraud data are missing, we assume fraud is detected.
    fn fraud_checks_strict(&self) -> [bool; N_FRAUD_CHECKS] {
        self.fraud_checks().map(|v| v.unwrap_or(true))
    }

    fn enabled_checks_from_config(_config: &BackendConfig) -> [bool; N_FRAUD_CHECKS] {
        []
    }

    fn feedback_messages() -> [Option<PipelineFailureFeedbackMessage>; N_FRAUD_CHECKS] {
        []
    }

    /// Get the || result of all fraud checks, but under the Orb configuration.
    /// The end result might be different from the || of all fraud booleans as
    /// we might decide to not block a signup even if it's fraudulent.
    #[must_use]
    pub fn fraud_detected_with_config(
        &self,
        config: &BackendConfig,
    ) -> (bool, Vec<PipelineFailureFeedbackMessage>) {
        let enabled_checks = Self::enabled_checks_from_config(config);
        let fraud_results = self.fraud_checks_strict();
        let feedback_msgs = Self::feedback_messages();

        let feedback: Vec<PipelineFailureFeedbackMessage> = enabled_checks
            .iter()
            .zip(fraud_results.iter())
            .zip(feedback_msgs.iter())
            .filter_map(
                |((&enabled, &result), feedback_msg)| {
                    if enabled && result { feedback_msg.clone() } else { None }
                },
            )
            .collect();

        (!feedback.is_empty(), feedback)
    }

    /// If any fraud check fails or is missing data, fraud is reported.
    #[must_use]
    pub fn fraud_detected(&self) -> bool {
        self.fraud_checks_strict().iter().any(|&v| v)
    }

    /// Report fraud checks as Datadog tags.
    pub fn as_datadog_tags(&self) -> impl Iterator<Item = String> {
        Self::DATADOG_TAGS.iter().zip(self.fraud_checks()).map(|(tag, res)| {
            format!("{}{}", tag, res.map_or("none".to_owned(), |b| b.to_string()))
        })
    }

    /// Report fraud checks as Datadog tags, but exclude reports of any fraud
    /// check that is not enabled in config.
    pub fn as_datadog_tags_with_config(
        &self,
        config: &BackendConfig,
    ) -> impl Iterator<Item = String> {
        self.as_datadog_tags()
            .zip(Self::enabled_checks_from_config(config))
            .filter_map(|(tag, is_enabled)| is_enabled.then_some(tag))
    }
}

/// Fraud checks plan.
#[derive(Debug)]
pub struct FraudChecks<'a> {
    _phantom: PhantomData<&'a ()>,
}

impl<'a> FraudChecks<'a> {
    /// Create a new FraudCheck.
    #[must_use]
    pub fn new(_pipeline: &'a biometric_pipeline::Pipeline) -> Self {
        Self { _phantom: PhantomData }
    }

    /// Run all fraud checks.
    #[must_use]
    pub fn run(&mut self) -> Report {
        Report {}
    }
}
