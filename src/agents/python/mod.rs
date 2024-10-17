//! Python-based agents.

use crate::{dd_incr, utils::RkyvNdarray};
use agentwire::{
    agent,
    port::{self, SharedPort, SharedSerializer},
};
use eyre::{Report, Result, WrapErr};
use ndarray::{Ix1, Ix2};
use numpy::{PyArray1, PyArray2};
use pyo3::prelude::*;
use regex::Regex;
use rkyv::{de::deserializers::SharedDeserializeMap, Archive, Deserialize, Infallible, Serialize};
use std::{collections::HashMap, ffi::CString};

pub mod face_identifier;
pub mod ir_net;
pub mod iris;
pub mod mega_agent_one;
pub mod mega_agent_two;
pub mod occlusion;
pub mod rgb_net;

// TODO(andronat): These marcos should go to all agents.

/// Helper macro to invoke Python's get_item() with proper eyre error handling.
#[macro_export]
macro_rules! get_item {
    ($o:ident, $key:expr) => {{
        $o.get_item($key).wrap_err_with(|| {
            format!(
                "{}:{}:{} get_item!(\"{}\", {}) failure",
                file!(),
                line!(),
                column!(),
                stringify!($o),
                $key
            )
        })
    }};
}

/// Helper macro to invoke Python's get_item() and then extract() with proper eyre error handling.
#[macro_export]
macro_rules! get_and_extract {
    ($o:ident, $key:expr) => {{
        get_item!($o, $key).and_then(|o| {
            o.extract().wrap_err_with(|| {
                format!(
                    "{}:{}:{} get_and_extract!(\"{}\", {}) failure",
                    file!(),
                    line!(),
                    column!(),
                    stringify!($o),
                    $key
                )
            })
        })
    }};
}

/// Helper to choose the config based on the model version or check for a global config.
fn choose_config(configs: Option<&HashMap<String, String>>, version: &str) -> Result<String> {
    let Some(configs) = configs else {
        eyre::bail!("No config found for `global` or for version: {version}.")
    };
    if let Some(config) = configs.get(version) {
        Ok(config.clone())
    } else if let Some(config) = configs.get("global") {
        tracing::warn!("No specific config found for: {version}. Using global config.");
        Ok(config.clone())
    } else {
        eyre::bail!("No config found for `global` or for version: {version}.")
    }
}

/// Helper to check if the model version is compatible with the agent. It returns the model version as a string.
fn check_model_version(module: &PyModule, min_version: &str) -> Result<String> {
    let version = module
        .getattr("__version__")?
        .extract::<String>()
        .unwrap_or_else(|_| panic!("{module:?} model must have a version"));

    let sem_req = semver::VersionReq::parse(format!(">={min_version}").as_str())
        .expect("predefined version requirement to be valid");
    let sem_version =
        semver::Version::parse(version.as_str()).wrap_err("version is not semver parsable")?;
    if !sem_req.matches(&sem_version) {
        eyre::bail!(
            "Installed model: `{name}`, version: `{version}` is not compatible with: `{sem_req}`",
            name = module.name().unwrap_or("unknown")
        );
    }
    Ok(version)
}

/// Python-based agent.
///
/// NOTE: When implementing this trait, add a new match arm to the
/// [`crate::agents::call_process_agent`].
#[allow(clippy::module_name_repetitions)] // for consistency with other agent traits
pub trait AgentPython: agent::Process + SharedPort
where
    <Self as Archive>::Archived: Deserialize<Self, Infallible>,
    Self::Input: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    Self::Output: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    <Self::Output as Archive>::Archived: Deserialize<Self::Output, SharedDeserializeMap>,
{
    /// Datadog reporting namespace.
    const DD_NS: &'static str;

    /// Minimum model version supported by this agent.
    const MINIMUM_MODEL_VERSION: &'static str;

    /// This method is called once, before the main loop.
    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn Environment<Self> + 'py>>;

    /// Helper function to report PyErr exceptions.
    fn report_python_exception(py: Python, err: &Report, pyerr: &PyErr) {
        tracing::warn!("Python agent '{}' python exception: {err:#?}", Self::NAME);
        // TODO(andronat): I think the following prints nothing!
        pyerr.print(py);

        let re = Regex::new(r"[^A-Za-z0-9]").unwrap();
        let s = pyerr.get_type(py).to_string();
        let sanitized_err_type = re.replace_all(&s, "_");

        dd_incr!(
            "main.count.neural_network" + format!("{}.python_exception.type", Self::DD_NS),
            &format!("type:{sanitized_err_type}")
        );
    }

    /// Runs the agent in a separate process.
    fn run_python_process(self, mut port: port::RemoteInner<Self>) -> Result<()> {
        Python::with_gil(|py| {
            init_sys_argv(py);
            tracing::info!("Python agent '{}': initializing", Self::NAME);
            let mut environment = self
                .init(py)
                .inspect_err(|err| {
                    if let Some(err) = err.downcast_ref::<PyErr>() {
                        tracing::info!("Python agent '{}' python exception: {err:#?}", Self::NAME);
                        err.print(py);
                    }
                })
                .wrap_err("python agent initialization")?;
            tracing::info!("Python agent '{}': environment loaded successfully", Self::NAME);
            loop {
                // Create a new pool, so that PyO3 can clear memory at the end of the loop.
                let pool = unsafe { py.new_pool() };
                let py = pool.python();

                let input = port.recv();
                let chain = input.chain_fn();

                match environment.iterate(py, input.value) {
                    Ok(output) => {
                        port.send(&chain(output));
                    }
                    Err(err) => {
                        tracing::warn!("Python agent '{}' error: {err:#?}", Self::NAME);
                        if let Some(err) = err.downcast_ref::<PyErr>() {
                            err.print(py);
                        }
                        break Err(err);
                    }
                }
            }
        })
    }
}

/// Python environment.
pub trait Environment<T: SharedPort>
where
    T::Input: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    T::Output: Archive + for<'a> Serialize<SharedSerializer<'a>>,
    <T::Output as Archive>::Archived: Deserialize<T::Output, SharedDeserializeMap>,
{
    /// This method is called continuously by the main loop.
    fn iterate(&mut self, py: Python, input: &<T::Input as Archive>::Archived)
    -> Result<T::Output>;
}

/// hack to fix module 'sys' has no attribute 'argv' error
pub fn init_sys_argv(_py: Python) {
    let string = CString::default();
    unsafe {
        let mut args = [pyo3::ffi::Py_DecodeLocale(string.as_ptr(), std::ptr::null_mut::<isize>())];
        pyo3::ffi::PySys_SetArgv(1, args.as_mut_ptr());
    }
}

fn extract_rkyv_ndarray_d1(obj: &PyAny) -> PyResult<RkyvNdarray<u32, Ix1>> {
    let arr: &PyArray1<u32> = obj.extract()?;
    Ok(RkyvNdarray::from(arr.to_owned_array()))
}

fn extract_normalized_iris(obj: &PyAny) -> PyResult<RkyvNdarray<u8, Ix2>> {
    let arr: &PyArray2<u8> = obj.extract()?;
    Ok(RkyvNdarray::from(arr.to_owned_array()))
}

fn extract_normalized_mask(obj: &PyAny) -> PyResult<RkyvNdarray<bool, Ix2>> {
    let arr: &PyArray2<bool> = obj.extract()?;
    Ok(RkyvNdarray::from(arr.to_owned_array()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentwire::{port::Port, Agent as _};
    use eyre::Error;
    use pyo3::types::PyDict;
    use std::collections::HashMap;

    #[derive(Clone, Debug, Archive, Serialize, Deserialize)]
    struct Model {}
    impl SharedPort for Model {
        const SERIALIZED_INIT_SIZE: usize = 0;
        const SERIALIZED_INPUT_SIZE: usize = 0;
        const SERIALIZED_OUTPUT_SIZE: usize = 0;
    }
    impl Port for Model {
        type Input = ();
        type Output = ();

        const INPUT_CAPACITY: usize = 0;
        const OUTPUT_CAPACITY: usize = 0;
    }
    impl agentwire::Agent for Model {
        const NAME: &'static str = "test";
    }
    impl AgentPython for Model {
        const DD_NS: &'static str = "test";
        const MINIMUM_MODEL_VERSION: &'static str = "100.0.1";

        fn init<'py>(self, _py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
            unreachable!()
        }
    }
    impl agentwire::agent::Process for Model {
        type Error = Error;

        fn run(self, port: port::RemoteInner<Self>) -> Result<(), Self::Error> {
            self.run_python_process(port)
        }
    }

    fn create_mock_module<'py>(py: Python<'py>, version: &str) -> PyResult<&'py PyModule> {
        let mock_module = PyModule::new(py, Model::NAME)?;
        mock_module.setattr("__version__", version)?;
        let sys_modules =
            PyModule::import(py, "sys")?.getattr("modules")?.downcast::<PyDict>().unwrap();
        sys_modules.set_item("my_module", mock_module)?;
        py.import("my_module")
    }

    /// Test that the model version is compatible with the agent.
    #[test]
    fn test_sem_ver_is_ok() -> Result<()> {
        Python::with_gil(|py| {
            let module = create_mock_module(py, "100.0.1")?;
            let res = check_model_version(module, Model::MINIMUM_MODEL_VERSION)?;
            assert_eq!(res, "100.0.1");
            Ok(())
        })
    }

    /// Test that the model version is not compatible with the agent.
    #[test]
    fn test_sem_ver_too_old() -> Result<()> {
        Python::with_gil(|py| {
            let module = create_mock_module(py, "99.0.1")?;
            let res = check_model_version(module, Model::MINIMUM_MODEL_VERSION);
            assert!(res.is_err());
            assert_eq!(
                res.err().unwrap().to_string(),
                "Installed model: `test`, version: `99.0.1` is not compatible with: `>=100.0.1`"
            );
            Ok(())
        })
    }

    #[test]
    fn test_sem_ver_not_parsing() {
        Python::with_gil(|py| {
            let module = create_mock_module(py, "abc").unwrap();
            let res = check_model_version(module, Model::MINIMUM_MODEL_VERSION);
            assert!(res.is_err());
            assert_eq!(res.err().unwrap().to_string(), "version is not semver parsable");
        });
    }

    #[test]
    fn test_choose_config_specific_version() -> Result<()> {
        let configs = HashMap::from([
            ("1.0".to_owned(), "Config for 1.0".to_owned()),
            ("global".to_owned(), "Global Config".to_owned()),
        ]);
        let config = choose_config(Some(&configs), "1.0")?;
        assert_eq!(config, "Config for 1.0");
        Ok(())
    }

    #[test]
    fn test_choose_config_global_fallback() -> Result<()> {
        let configs = HashMap::from([("global".to_owned(), "Global Config".to_owned())]);
        let config = choose_config(Some(&configs), "2.0")?;
        assert_eq!(config, "Global Config");
        Ok(())
    }

    #[test]
    fn test_choose_config_no_config() {
        let configs: Option<HashMap<String, String>> = None;
        let result = choose_config(configs.as_ref(), "1.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_choose_config_no_matching_version() {
        let configs = HashMap::from([("1.0".to_owned(), "Config for 1.0".to_owned())]);
        let result = choose_config(Some(&configs), "2.0");
        assert!(result.is_err());
    }
}
