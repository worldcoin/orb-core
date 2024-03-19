//! Face identifier python agent.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv
#![allow(
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    clippy::unused_self,
    unused_variables
)]

/// Face identifier python agent types.
pub mod types;

pub use types::Bundle;

use self::types::{BackendConfig, Embedding, IsValidOutput, Thumbnail};
use crate::{
    agents::{
        camera,
        python::{rgb_net, AgentPython},
        Agent,
    },
    config::Config,
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
    inst_elapsed,
    logger::{LogOnError, DATADOG, NO_TAGS},
    port::{Port, SharedPort},
};
use eyre::Result;
use ndarray::{Array, Array3};
use pyo3::{prelude::*, types::PyDict};
use python_agent_interface::PyError;
use rkyv::{Archive, Deserialize, Infallible, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::time::Instant;

/// Face identifier python agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct Model {}

/// Agent input.
#[derive(Debug, Archive, Serialize)]
#[allow(clippy::large_enum_variant)]
pub enum Input {
    /// Face identifier similarity score.
    Estimate {
        /// Left face RGB frame.
        frame_left: camera::rgb::Frame,
        /// Right face RGB frame.
        frame_right: camera::rgb::Frame,
        /// The face RGB frame validated by the face model during biometric capture.
        frame_self_custody_candidate: camera::rgb::Frame,
        /// The eye landmarks of the left face RGB frame.
        eyes_landmarks_left: (rgb_net::Point, rgb_net::Point),
        /// The eye landmarks of the right face RGB frame.
        eyes_landmarks_right: (rgb_net::Point, rgb_net::Point),
        /// The face RGB frame eye landmarks, validated by the face model during biometric capture.
        eyes_landmarks_self_custody_candidate: (rgb_net::Point, rgb_net::Point),
        /// The bbox of the left face RGB frame.
        bbox_left: rgb_net::Rectangle,
        /// The bbox of the right face RGB frame.
        bbox_right: rgb_net::Rectangle,
        /// The bbox of the self-custody face RGB frame.
        bbox_self_custody_candidate: rgb_net::Rectangle,
    },
    /// Face identifier is valid image, used in biometric capture.
    IsValid {
        /// Face RGB frame.
        frame: camera::rgb::Frame,
        /// Eye landmarks from RGB-Net.
        eyes_landmarks: (rgb_net::Point, rgb_net::Point),
        /// Eye landmarks from RGB-Net.
        bbox: rgb_net::Rectangle,
    },
    /// Update the agent config.
    UpdateConfig(BackendConfig),
    /// Warmup the model with a dummy call.
    Warmup,
}

/// Agent output.
#[derive(Debug, Archive, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Output {
    /// Face identifier bundle generation.
    Estimate {
        /// Face identifier bundle that includes the face identifier image and embeddings.
        bundle: Bundle,
    },
    /// Face identifier is valid image, used in biometric capture.
    IsValidImage(IsValidOutput),
    /// UpdateConfig response.
    UpdateConfig,
    /// Warmup call response.
    Warmup,
    /// Face identifier returned with a Python exception error.
    Error(PyError),
}

/// Environment wrapper for the python agent.
pub struct Environment<'py> {
    /// The python agent itself.
    pub agent: &'py PyAny,
}

impl Port for Model {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Model {
    const SERIALIZED_CONFIG_EXTRA_SIZE: usize = 8192;
    const SERIALIZED_INPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096;
}

impl super::Agent for Model {
    const NAME: &'static str = "face-identifier";
}

impl super::Environment<Model> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        let t = Instant::now();

        let (op, res) = match input {
            ArchivedInput::Estimate {
                frame_left,
                frame_right,
                frame_self_custody_candidate,
                eyes_landmarks_left,
                eyes_landmarks_right,
                eyes_landmarks_self_custody_candidate,
                bbox_left,
                bbox_right,
                bbox_self_custody_candidate,
            } => (
                "estimate",
                self.estimate(
                    py,
                    frame_left,
                    frame_right,
                    frame_self_custody_candidate,
                    eyes_landmarks_left,
                    eyes_landmarks_right,
                    eyes_landmarks_self_custody_candidate,
                    bbox_left,
                    bbox_right,
                    bbox_self_custody_candidate,
                ),
            ),
            ArchivedInput::IsValid { frame, eyes_landmarks, bbox } => (
                "is_valid",
                self.is_valid(
                    py,
                    frame.into_ndarray(),
                    eyes_landmarks.deserialize(&mut Infallible)?,
                    bbox.deserialize(&mut Infallible)?,
                )
                .map(Output::IsValidImage),
            ),
            ArchivedInput::UpdateConfig(backend_config) => (
                "update_config",
                self.update_config(&backend_config.deserialize(&mut Infallible)?)
                    .map(|()| Output::UpdateConfig),
            ),
            ArchivedInput::Warmup => ("warmup", self.warmup().map(|()| Output::Warmup)),
        };

        DATADOG
            .timing(
                format!("orb.main.time.processing.{}.{}", Model::DD_NS, op),
                inst_elapsed!(t),
                NO_TAGS,
            )
            .or_log();
        tracing::info!("Python agent {}::{} <benchmark>: {} ms", Model::NAME, op, inst_elapsed!(t));

        res.or_else(|e| {
            if let Some(pe) = e.downcast_ref::<PyErr>() {
                <Model as super::AgentPython>::report_python_exception(py, &e, pe);
                Ok(Output::Error(PyError::from_py_err(pe, py)))
            } else {
                Err(e)
            }
        })
    }
}

impl Environment<'_> {
    /// Create a new python agent environment.
    pub fn new<'a>(py: Python<'a>, configs: &'_ Model) -> Result<Environment<'a>> {
        tracing::info!("{} agent: loading model with config: {:?}", Model::NAME, configs);
        let t = Instant::now();

        tracing::info!(
            "Python agent {} <benchmark>: initialization done in {} ms",
            Model::NAME,
            inst_elapsed!(t)
        );
        DATADOG
            .timing(
                format!("orb.main.time.neural_network.init.{}", Model::DD_NS),
                inst_elapsed!(t),
                NO_TAGS,
            )
            .or_log();

        Ok(Environment { agent: PyDict::new(py) })
    }

    fn warmup(&self) -> Result<()> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn estimate(
        &self,
        py: Python,
        left: &camera::rgb::ArchivedFrame,
        right: &camera::rgb::ArchivedFrame,
        self_custody_candidate: &camera::rgb::ArchivedFrame,
        eyes_landmarks_left: &(rgb_net::ArchivedPoint, rgb_net::ArchivedPoint),
        eyes_landmarks_right: &(rgb_net::ArchivedPoint, rgb_net::ArchivedPoint),
        eyes_landmarks_self_custody_candidate: &(rgb_net::ArchivedPoint, rgb_net::ArchivedPoint),
        bbox_left: &rgb_net::ArchivedRectangle,
        bbox_right: &rgb_net::ArchivedRectangle,
        bbox_self_custody_candidate: &rgb_net::ArchivedRectangle,
    ) -> Result<Output> {
        Ok(Output::Estimate {
            bundle: Bundle {
                error: None,
                thumbnail: Some(Thumbnail {
                    border: None,
                    bounding_box: None,
                    image: Some(self_custody_candidate.into_ndarray().into()),
                    rotated_angle: None,
                    shape: Some((100, 100, 3)),
                    original_shape: None,
                    original_image: None,
                }),
                embeddings: Some(vec![Embedding {
                    embedding: Array::from_shape_vec(1, vec![0]).unwrap().into(),
                    embedding_type: "orb-core-base".into(),
                    embedding_version: "orb-core-base".into(),
                    embedding_inference_backend: "orb-core-base".into(),
                }]),
                inference_backend: Some("orb-core-base".into()),
            },
        })
    }

    /// Check if the RGB face image meets our quality standards.
    pub fn is_valid(
        &self,
        py: Python,
        frame: Array3<u8>,
        rgb_net_eye_landmarks: (rgb_net::Point, rgb_net::Point),
        rgb_net_bbox: rgb_net::Rectangle,
    ) -> Result<IsValidOutput> {
        Ok(IsValidOutput {
            error: None,
            inference_backend: Some("orb-core-base".into()),
            is_valid: Some(true),
            score: Some(1.0),
            rgb_net_eye_landmarks,
            rgb_net_bbox,
        })
    }

    fn update_config(&mut self, _configs: &BackendConfig) -> Result<()> {
        Ok(())
    }
}

impl super::AgentPython for Model {
    const DD_NS: &'static str = "face_identifier";
    const MINIMUM_MODEL_VERSION: &'static str = "orb-core-base";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        Ok(Box::new(Environment::new(py, &self)?))
    }
}

impl From<&Config> for Model {
    fn from(config: &Config) -> Self {
        Self {}
    }
}
