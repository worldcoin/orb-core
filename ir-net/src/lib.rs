//! IR-Net bindings.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use ai_interface::InitAgent;
use eyre::{Result, WrapErr};
use numpy::PyArray2;
use pyo3::{prelude::*, types::PyDict};

/// IR-Net.
pub struct IrNet<'p> {
    py: Python<'p>,
    agent: &'p PyAny,
}

impl<'p> IrNet<'p> {
    /// Initializes a new [`IrNet`].
    #[allow(clippy::missing_panics_doc)]
    pub fn init(py: Python<'p>, config: &String) -> Result<Self> {
        let ir_net = py.import("ir_net")?;
        let init: InitAgent =
            ir_net.getattr("IRNet")?.getattr("init_from_config")?.call1((config,))?.extract()?;
        let agent =
            init.agent.ok_or_else(|| init.error.expect("error should exist if agent is None"))?;

        Ok(Self { py, agent })
    }

    /// Returns the Python module.
    pub fn module(py: Python<'p>) -> PyResult<&PyModule> {
        py.import("ir_net")
    }

    /// Estimates landmarks, pupil-to-iris ratio, occlusion values and
    /// fractional LaPlace sharpness of a single image of the human eye.
    pub fn estimate(
        &self,
        image: &PyArray2<u8>,
        target_left_eye: bool,
        focus_matrix_code: bool,
    ) -> Result<&PyAny> {
        let kwargs = PyDict::new(self.py);
        if focus_matrix_code {
            kwargs.set_item("focus_matrix_code", true)?;
        }
        kwargs.set_item("target_side", i32::from(!target_left_eye))?;
        self.agent
            .call_method("estimate", (image,), Some(kwargs))
            .wrap_err("IrNet estimate call failed")
    }
}
