//! Mega Agent One. A single Python process that loads multiple AI models.
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
        python::{ir_net, iris},
        Agent,
    },
    config::Config,
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
    inst_elapsed,
    port::{Port, SharedPort},
};
use eyre::Result;
use pyo3::Python;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::{mem::size_of, time::Instant};

/// Mega Agent One.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
#[schemars(rename = "MegaAgentOneConfig")]
pub struct MegaAgentOne {
    /// Initial state for the IR-Net model.
    pub ir_net: ir_net::Model,
    /// Initial state for the Iris model.
    pub iris: iris::Model,
}

impl super::Agent for MegaAgentOne {
    const NAME: &'static str = "mega-agent-one";
}

/// Input that Orb Core provides to the Mega Agent to be forwarded to the
/// appropriate AI model.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// Input for the Iris model.
    Iris(iris::Input),
    /// Input for the IR-Net model.
    IRNet(ir_net::Input),
    /// The configuration of the Mega Agent One.
    Config,
}

/// Output wrapper for each of our AI models.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Output {
    /// Output for the Iris model.
    Iris(iris::Output),
    /// Output for the IR-Net model.
    IRNet(ir_net::Output),
    /// The configuration of the Mega Agent One.
    Config(MegaAgentOne),
}

impl Port for MegaAgentOne {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 15;
    const OUTPUT_CAPACITY: usize = 15;
}

impl SharedPort for MegaAgentOne {
    const SERIALIZED_CONFIG_EXTRA_SIZE: usize =
        <ir_net::Model as SharedPort>::SERIALIZED_CONFIG_EXTRA_SIZE
            + <iris::Model as SharedPort>::SERIALIZED_CONFIG_EXTRA_SIZE;
    const SERIALIZED_INPUT_SIZE: usize = size_of::<<Input as Archive>::Archived>()
        + (RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3) * 2;
    const SERIALIZED_OUTPUT_SIZE: usize = size_of::<<Output as Archive>::Archived>()
        + (RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3);
}

struct Environment<'py> {
    iris_env: Box<dyn super::Environment<iris::Model> + 'py>,
    ir_net_env: Box<dyn super::Environment<ir_net::Model> + 'py>,
    config: MegaAgentOne,
}

impl super::Environment<MegaAgentOne> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        match input {
            ArchivedInput::Iris(input) => {
                tracing::debug!("{}: Received input for Iris", MegaAgentOne::NAME);
                Ok(Output::Iris(self.iris_env.iterate(py, input)?))
            }
            ArchivedInput::IRNet(input) => {
                // This is at Trace level as we expect lots of inputs.
                tracing::trace!("{}: Received input for IR-Net", MegaAgentOne::NAME);
                Ok(Output::IRNet(self.ir_net_env.iterate(py, input)?))
            }
            ArchivedInput::Config => {
                tracing::debug!("{}: Received input for Config", MegaAgentOne::NAME);
                Ok(Output::Config(self.config.clone()))
            }
        }
    }
}

impl super::AgentPython for MegaAgentOne {
    const DD_NS: &'static str = "mega_agent_one";
    const MINIMUM_MODEL_VERSION: &'static str = "";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{}: initializing all models", MegaAgentOne::NAME);
        let config_clone = self.clone();
        let t = Instant::now();

        let iris_env = iris::Model::init(self.iris, py)?;
        let ir_net_env = ir_net::Model::init(self.ir_net, py)?;

        tracing::info!(
            "Python agent {} <benchmark>: initialization done in {} ms",
            MegaAgentOne::NAME,
            inst_elapsed!(t)
        );

        Ok(Box::new(Environment { iris_env, ir_net_env, config: config_clone }))
    }
}

impl From<&Config> for MegaAgentOne {
    fn from(config: &Config) -> Self {
        Self { iris: config.into(), ir_net: config.into() }
    }
}
