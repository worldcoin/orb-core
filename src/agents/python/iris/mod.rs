//! Iris python agent.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

mod extracts;
/// Iris model python agent types.
pub mod types;

#[allow(clippy::module_name_repetitions)]
pub use self::types::{IrisTemplate, Metadata, NormalizedIris, PipelineOutput};
use crate::{
    agents::{
        camera,
        python::{check_model_version, choose_config, AgentPython},
        ProcessInitializer,
    },
    config::Config,
    consts::{IR_HEIGHT, IR_WIDTH},
    dd_timing,
    utils::log_iris_data,
};
use agentwire::{
    agent::{self, Agent as _},
    port::{self, Port, SharedPort},
};
use ai_interface::{InitAgent, PyError};
use eyre::{Error, Result};
use iris_mpc::{galois_engine::degree4::GaloisRingIrisCodeShare, iris_db::iris::IrisCodeArray};
use numpy::PyArray2;
use pyo3::{types::PyDict, PyAny, PyErr, Python};
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::{collections::HashMap, str, time::Instant};

/// Iris python agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
#[cfg_attr(feature = "stage", derive(Debug))]
pub struct Model {
    configs: Option<HashMap<String, String>>,
}

#[cfg(not(feature = "stage"))]
impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "*Redacted in prod: get the config from debug-report. Use 'stage' for printing*")
    }
}

/// Agent input.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// Iris estimate function.
    Estimate {
        /// IR frame.
        frame: camera::ir::Frame,
        /// Whether the iris belongs to the left eye.
        left_eye: bool,
    },
    /// Get Iris version.
    Version,
}

/// Agent output.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Output {
    /// Iris estimate function.
    Estimate(EstimateOutput),
    /// Iris version.
    Version(String),
    /// Iris returned with a Python exception error.
    Error(PyError),
}

/// Iris estimate output.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct EstimateOutput {
    /// The Iris code shares.
    pub iris_code_shares: [String; 3],
    /// The Iris mask code shares.
    pub mask_code_shares: [String; 3],
    /// The Iris code.
    pub iris_code: String,
    /// The Iris mask code.
    pub mask_code: String,
    /// The Iris code version.
    pub iris_code_version: String,
    /// The Iris metadata.
    pub metadata: Metadata,
    /// The Iris normalized image.
    pub normalized_image: Option<NormalizedIris>,
    /// The Iris resized normalized image.
    pub normalized_image_resized: Option<NormalizedIris>,
}

impl TryFrom<PipelineOutput> for EstimateOutput {
    type Error = PyError;

    fn try_from(output: PipelineOutput) -> std::result::Result<Self, Self::Error> {
        if let Some(iris_template) = output.iris_template {
            let iris_code = IrisCodeArray::from_base64(&iris_template.iris_codes)?;
            let mask_code = IrisCodeArray::from_base64(&iris_template.mask_codes)?;

            let iris_code_shares = GaloisRingIrisCodeShare::encode_iris_code(
                &iris_code,
                &mask_code,
                &mut rand::thread_rng(),
            )
            .map(|x| x.to_base64());
            let mask_code_shares =
                GaloisRingIrisCodeShare::encode_mask_code(&mask_code, &mut rand::thread_rng())
                    .map(|x| x.to_base64());

            Ok(EstimateOutput {
                iris_code_shares,
                mask_code_shares,
                iris_code: iris_template.iris_codes,
                mask_code: iris_template.mask_codes,
                iris_code_version: iris_template.iris_code_version,
                metadata: output.metadata,
                normalized_image: output.normalized_image,
                normalized_image_resized: output.normalized_image_resized,
            })
        } else {
            Err(output.error.expect("error not to be None"))
        }
    }
}

impl Port for Model {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Model {
    const SERIALIZED_INIT_SIZE: usize = 16384 * 20;
    const SERIALIZED_INPUT_SIZE: usize = 4096 + IR_HEIGHT as usize * IR_WIDTH as usize;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096 + 16 * 200 * 2 * 2;
}

impl agentwire::Agent for Model {
    const NAME: &'static str = "iris";
}

struct Environment<'py> {
    agent: &'py PyAny,
    version: String,
}

impl super::Environment<Model> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        let t = Instant::now();

        let (op, res) = match input {
            ArchivedInput::Estimate { frame, left_eye } => {
                ("estimate", self.estimate(py, frame, *left_eye).map(Output::Estimate))
            }
            ArchivedInput::Version => ("version", Ok(Output::Version(self.version.clone()))),
        };

        dd_timing!("main.time.processing" + format!("{}.{}", Model::DD_NS, op), t);
        tracing::info!(
            "Python agent {}::{} <benchmark>: {} ms",
            Model::NAME,
            op,
            t.elapsed().as_millis()
        );

        res.or_else(|e| {
            if let Some(pe) = e.downcast_ref::<PyErr>() {
                <Model as super::AgentPython>::report_python_exception(py, &e, pe);
                Ok(Output::Error(PyError::from_py_err(pe, py)))
            } else if let Some(pe) = e.downcast_ref::<PyError>() {
                Ok(Output::Error(pe.clone()))
            } else {
                Err(e)
            }
        })
    }
}

impl Environment<'_> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn estimate(
        &self,
        py: Python,
        frame: &camera::ir::ArchivedFrame,
        left_eye: bool,
    ) -> Result<EstimateOutput> {
        let image = PyArray2::from_owned_array(py, frame.into_ndarray());

        let kwargs = PyDict::new(py);
        if left_eye {
            kwargs.set_item("eye_side", "left")?;
        } else {
            kwargs.set_item("eye_side", "right")?;
        };

        let output: EstimateOutput = self
            .agent
            .call_method("estimate", (image,), Some(kwargs))?
            .extract::<PipelineOutput>()?
            .try_into()?;

        log_iris_data(
            &output.iris_code_shares,
            &output.mask_code_shares,
            &output.iris_code,
            &output.mask_code,
            &output.iris_code_version,
            left_eye,
            "iris agent",
        );
        Ok(output)
    }
}

impl super::AgentPython for Model {
    const DD_NS: &'static str = "iris";
    const MINIMUM_MODEL_VERSION: &'static str = "1.7.4";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{} agent: loading model with config: {self:?}", Model::NAME);
        let t = Instant::now();

        let module = py.import("iris")?;
        let version = check_model_version(module, Model::MINIMUM_MODEL_VERSION)?;
        let config = choose_config(self.configs.as_ref(), &version)?;

        let module = py.import("iris.pipelines.iris_pipeline")?;
        let init: InitAgent = module
            .getattr("IRISPipeline")?
            .getattr("load_from_config")?
            .call1((config,))?
            .extract()?;
        let agent =
            init.agent.ok_or_else(|| init.error.expect("error should exist if agent is None"))?;

        tracing::info!(
            "Python agent {} <benchmark>: initialization done in {} ms",
            Model::NAME,
            t.elapsed().as_millis()
        );
        dd_timing!("main.time.neural_network.init" + format!("{}", Model::DD_NS), t);
        Ok(Box::new(Environment { agent, version }))
    }
}

impl agentwire::agent::Process for Model {
    type Error = Error;

    fn run(self, port: port::RemoteInner<Self>) -> Result<(), Self::Error> {
        self.run_python_process(port)
    }

    fn initializer() -> impl agent::process::Initializer {
        ProcessInitializer::default()
    }
}

impl From<&Config> for Model {
    fn from(config: &Config) -> Self {
        Self { configs: config.iris_model_configs.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::{
        pyfunction,
        types::{PyCFunction, PyModule},
        wrap_pyfunction, PyObject, PyResult, ToPyObject,
    };

    fn create_iris_module(
        py: Python,
        iris_version: &str,
        pyfunction: &PyCFunction,
    ) -> PyResult<()> {
        // Mock the Iris module.
        let iris_mock = PyModule::new(py, "iris")?;
        iris_mock.setattr("__version__", iris_version)?;
        // Create all submodules and classes.
        let pipelines_mock = PyModule::new(py, "pipelines")?;
        let iris_pipeline_mock = PyModule::new(py, "iris_pipeline")?;
        let iris_pipeline_class = PyModule::new(py, "IRISPipeline")?;
        // Add all submodules and classes to the Iris module.
        iris_mock.add_submodule(pipelines_mock)?;
        pipelines_mock.add_submodule(iris_pipeline_mock)?;
        iris_pipeline_class.add_function(pyfunction)?;
        iris_pipeline_mock.add("IRISPipeline", iris_pipeline_class)?;
        // Add the Iris module to the sys.modules.
        let sys_modules =
            PyModule::import(py, "sys")?.getattr("modules")?.downcast::<PyDict>().unwrap();
        sys_modules.set_item("iris", iris_mock)?;
        sys_modules.set_item("iris.pipelines", pipelines_mock)?;
        sys_modules.set_item("iris.pipelines.iris_pipeline", iris_pipeline_mock)?;
        Ok(())
    }

    /// Can we create an Iris model with an explicit config?
    #[test]
    #[allow(unsafe_op_in_unsafe_fn)]
    fn test_specific_config() -> Result<()> {
        #[pyfunction]
        fn load_from_config(config: &PyAny) -> PyResult<PyObject> {
            let py = config.py();

            assert_eq!(config.extract::<String>()?, "specific");

            let response = PyDict::new(py);
            response.set_item("agent", "OK")?;
            response.set_item("error", py.None())?;
            Ok(response.to_object(py))
        }

        Python::with_gil(|py| {
            create_iris_module(py, "100.0.1", wrap_pyfunction!(load_from_config, py)?)?;
            let configs = HashMap::from([
                ("100.0.1".to_owned(), "specific".to_owned()),
                ("global".to_owned(), "generic".to_owned()),
            ]);
            let _iris_env = Model::init(Model { configs: Some(configs) }, py)?;
            Ok(())
        })
    }

    /// Can we create an Iris model with a generic config?
    #[test]
    #[allow(unsafe_op_in_unsafe_fn)]
    fn test_generic_config() -> Result<()> {
        #[pyfunction]
        fn load_from_config(config: &PyAny) -> PyResult<PyObject> {
            let py = config.py();

            assert_eq!(config.extract::<String>()?, "generic");

            let response = PyDict::new(py);
            response.set_item("agent", "OK")?;
            response.set_item("error", py.None())?;
            Ok(response.to_object(py))
        }

        Python::with_gil(|py| {
            create_iris_module(py, "100.0.1", wrap_pyfunction!(load_from_config, py)?)?;
            let configs = HashMap::from([
                ("100.0.2".to_owned(), "specific".to_owned()),
                ("global".to_owned(), "generic".to_owned()),
            ]);
            let _iris_env = Model::init(Model { configs: Some(configs) }, py)?;
            Ok(())
        })
    }

    /// If no config, please fail.
    #[test]
    #[allow(unsafe_op_in_unsafe_fn)]
    fn test_no_config() -> Result<()> {
        #[pyfunction]
        fn load_from_config(_config: &PyAny) -> PyResult<PyObject> {
            unreachable!("We should not reach this point");
            #[allow(unreachable_code)]
            Err(PyErr::new::<PyAny, &str>(""))
        }

        Python::with_gil(|py| {
            create_iris_module(py, "100.0.1", wrap_pyfunction!(load_from_config, py)?)?;
            let configs = HashMap::from([
                ("100.0.2".to_owned(), "specific".to_owned()),
                ("100.0.5".to_owned(), "generic".to_owned()),
            ]);
            assert!(Model::init(Model { configs: Some(configs) }, py).is_err());
            Ok(())
        })
    }
}
