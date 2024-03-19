//! RGB-Net bindings.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use numpy::PyArray3;
use pyo3::{prelude::*, types::PyDict};

/// IR-Net.
pub struct RgbNet<'p> {
    rgb_net: &'p PyModule,
    agent: &'p PyAny,
}

impl<'p> RgbNet<'p> {
    /// Initializes a new [`RgbNet`].
    pub fn init(py: Python<'p>) -> PyResult<Self> {
        let rgb_net = py.import("rgb_net")?;
        let kwargs = PyDict::new(py);
        let agent = rgb_net.getattr("RGBNet")?.call((), Some(kwargs))?;
        Ok(Self { rgb_net, agent })
    }

    /// Returns the Python module.
    #[must_use]
    pub fn module(&self) -> &PyModule {
        self.rgb_net
    }

    /// Returns IR-Net version.
    pub fn version(&self) -> PyResult<String> {
        self.rgb_net.getattr("__version__")?.extract()
    }

    /// Estimate the position of the eyes.
    pub fn estimate(&self, image: &PyArray3<u8>) -> PyResult<&PyAny> {
        self.agent.call_method1("estimate", (image,))
    }
}
