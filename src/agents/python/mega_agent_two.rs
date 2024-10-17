//! Mega Agent Two. A single Python process that loads multiple AI models.
//!
//! We have noticed that many of our AI models share the same libraries. Instead
//! of starting multiple processes for each model, and thus pressuring the Orb's
//! memory with isolated processes that load multiple times the same
//! Cuda/TensorRT libraries; the Mega Agent merges all of our models under a
//! single process. Python is smart enough to load each library only once. The
//! downside of the Mega Agent is that we lose parallelization. But given the
//! fact that our memory is limited and we load each model serially, this
//! solution offer a much better opportunity for other optimizations.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera,
        python::{face_identifier, iris, rgb_net, AgentPython},
        ProcessInitializer,
    },
    config::Config,
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
    dd_timing,
};
use agentwire::{
    agent::{self, Agent as _},
    port::{self, Port, SharedPort},
};
use ai_interface::PyError;
use eyre::{Error, Result};
use pyo3::{PyErr, Python};
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::{mem::size_of, time::Instant};

/// Mega Agent Two.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct MegaAgentTwo {
    /// Initial state for the Face Identifier model.
    face_identifier: face_identifier::Model,
    /// Initial state for the RGB-Net model.
    pub rgb_net: rgb_net::Model,
    /// Initial state for the Iris model.
    pub iris: iris::Model,
}

impl agentwire::Agent for MegaAgentTwo {
    const NAME: &'static str = "mega-agent-two";
}

/// Input that Orb Core provides to the Mega Agent to be forwarded to the
/// appropriate AI model.
#[derive(Debug, Archive, Serialize)]
#[allow(clippy::large_enum_variant)]
pub enum Input {
    /// Input for the Face Identifier model.
    FaceIdentifier(face_identifier::Input),
    /// Input for the RGB-Net model.
    RgbNet(rgb_net::Input),
    /// Input for the Iris model.
    Iris(iris::Input),
    /// Input for the RGB-Net and the Face Identifier model.
    FusionRgbNetFaceIdentifier {
        /// RGB frame.
        frame: camera::rgb::Frame,
    },
    /// The configuration of the Mega Agent Two.
    Config,
}

/// Output wrapper for each of our AI models.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Output {
    /// Output for the Face Identifier model.
    FaceIdentifier(face_identifier::Output),
    /// Output for the RGB-Net model.
    RgbNet(rgb_net::Output),
    /// Output for the Iris model.
    Iris(Box<iris::Output>),
    /// Output for the RGB-Net and the Face Identifier model.
    FusionRgbNetFaceIdentifier {
        /// Output for the RGB-Net model.
        rgb_net: rgb_net::EstimateOutput,
        /// Output for the Face Identifier model.
        face_identifier: face_identifier::types::IsValidOutput,
    },
    /// The configuration of the Mega Agent Two.
    Config(MegaAgentTwo),
    /// Propagate 'fusion' agents errors.
    FusionError(FusionErrors),
}

/// Errors that can be returned by the 'fusion' agents.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum FusionErrors {
    /// Error from the 'fusion' of RGB-Net and Face Identifier.
    RgbNetFaceIdentifier(Option<rgb_net::Output>, Option<face_identifier::Output>),
}

impl Port for MegaAgentTwo {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 2;
    const OUTPUT_CAPACITY: usize = 2;
}

impl SharedPort for MegaAgentTwo {
    const SERIALIZED_INIT_SIZE: usize = size_of::<usize>()
        + size_of::<<MegaAgentTwo as Archive>::Archived>()
        + <face_identifier::Model as SharedPort>::SERIALIZED_INIT_SIZE
        + <rgb_net::Model as SharedPort>::SERIALIZED_INIT_SIZE
        + <iris::Model as SharedPort>::SERIALIZED_INIT_SIZE;
    const SERIALIZED_INPUT_SIZE: usize = size_of::<<Input as Archive>::Archived>()
        + (RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3) * 4;
    const SERIALIZED_OUTPUT_SIZE: usize = size_of::<<Output as Archive>::Archived>()
        + (RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3) * 2;
}

struct Environment<'py> {
    face_identifier_env: face_identifier::Environment<'py>,
    rgb_net_env: rgb_net::Environment<'py>,
    iris_env: Box<dyn super::Environment<iris::Model> + 'py>,
    config: MegaAgentTwo,
}

impl super::Environment<MegaAgentTwo> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        match input {
            ArchivedInput::FaceIdentifier(input) => {
                tracing::debug!("{}: Received input for FaceIdentifier", MegaAgentTwo::NAME);
                self.face_identifier_env.iterate(py, input).map(Output::FaceIdentifier)
            }
            ArchivedInput::RgbNet(input) => {
                tracing::trace!("{}: Received input for RGB-Net", MegaAgentTwo::NAME);
                self.rgb_net_env.iterate(py, input).map(Output::RgbNet)
            }
            ArchivedInput::FusionRgbNetFaceIdentifier { frame } => {
                tracing::info!(
                    "{}: Received input for 'fusion' RGB-Net and FaceIdentifier",
                    MegaAgentTwo::NAME
                );
                let t = Instant::now();

                let res = self.fusion_rgb_net_face_identifier(py, frame).or_else(|e| {
                    if let Some(pe) = e.downcast_ref::<PyErr>() {
                        <MegaAgentTwo as super::AgentPython>::report_python_exception(py, &e, pe);
                        Ok(Output::FusionError(FusionErrors::RgbNetFaceIdentifier(
                            Some(rgb_net::Output::Error),
                            Some(face_identifier::Output::Error(PyError::from_py_err(pe, py))),
                        )))
                    } else {
                        Err(e)
                    }
                });

                let op = "fusion_rgbnet_faceidentifier";
                dd_timing!("main.time.processing" + format!("{}.{}", MegaAgentTwo::DD_NS, op), t);
                tracing::info!(
                    "Python agent {}::{} <benchmark>: {} ms",
                    MegaAgentTwo::NAME,
                    op,
                    t.elapsed().as_millis()
                );

                res
            }
            ArchivedInput::Iris(input) => {
                tracing::debug!("{}: Received input for Iris", MegaAgentTwo::NAME);
                Ok(Output::Iris(Box::new(self.iris_env.iterate(py, input)?)))
            }
            ArchivedInput::Config => {
                tracing::debug!("{}: Received input for Config", MegaAgentTwo::NAME);
                Ok(Output::Config(self.config.clone()))
            }
        }
    }
}

impl Environment<'_> {
    fn fusion_rgb_net_face_identifier(
        &mut self,
        py: Python,
        frame: &camera::rgb::ArchivedFrame,
    ) -> Result<Output> {
        let ndframe = frame.into_ndarray();

        let rgb_net_res = self.rgb_net_env.rgb_net_estimate(py, ndframe.clone())?;
        // If RGB-Net fails to estimate the eyes coordinates and bbox, avoid running the slow Face Identifier.
        let Some(prediction) = rgb_net_res.primary() else {
            return Ok(Output::FusionError(FusionErrors::RgbNetFaceIdentifier(
                Some(rgb_net::Output::Error),
                None,
            )));
        };
        let face_identifier_res = self.face_identifier_env.is_valid(
            py,
            ndframe,
            (prediction.landmarks.left_eye, prediction.landmarks.right_eye),
            prediction.bbox.coordinates,
        )?;

        Ok(Output::FusionRgbNetFaceIdentifier {
            rgb_net: rgb_net_res,
            face_identifier: face_identifier_res,
        })
    }
}

impl super::AgentPython for MegaAgentTwo {
    const DD_NS: &'static str = "mega_agent_two";
    const MINIMUM_MODEL_VERSION: &'static str = "";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{}: initializing all models", MegaAgentTwo::NAME);
        let config_clone = self.clone();
        let t = Instant::now();

        // The ROC's face model has to initialize first or else everything else fails.
        let face_identifier_env = face_identifier::Environment::new(py, &self.face_identifier)?;
        let rgb_net_env = rgb_net::Environment::new(py)?;
        let iris_env = iris::Model::init(self.iris, py)?;

        tracing::info!(
            "Python agent {} <benchmark>: initialization done in {} ms",
            MegaAgentTwo::NAME,
            t.elapsed().as_millis()
        );

        Ok(Box::new(Environment {
            face_identifier_env,
            rgb_net_env,
            iris_env,
            config: config_clone,
        }))
    }
}

impl agentwire::agent::Process for MegaAgentTwo {
    type Error = Error;

    fn run(self, port: port::RemoteInner<Self>) -> Result<(), Self::Error> {
        self.run_python_process(port)
    }

    fn initializer() -> impl agent::process::Initializer {
        ProcessInitializer::default()
    }
}

impl From<&Config> for MegaAgentTwo {
    fn from(config: &Config) -> Self {
        Self {
            rgb_net: rgb_net::Model::default(),
            iris: config.into(),
            face_identifier: config.into(),
        }
    }
}
