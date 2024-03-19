//! Rust <-> Python interface for Worldcoin's AI models.

use pyo3::{prelude::*, FromPyObject};
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use thiserror::Error;

/// Python Agent Error.
#[derive(
    FromPyObject, Error, Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[error("{:?}", self)]
pub struct PyError {
    /// Error type.
    pub error_type: String,
    /// Error message.
    pub message: String,
    /// Error traceback.
    pub traceback: String,
}

impl PyError {
    pub fn from_py_err(py_err: &PyErr, py: Python) -> Self {
        Self {
            error_type: py_err.get_type(py).to_string(),
            message: py_err.value(py).to_string(),
            traceback: py_err.traceback(py).map_or(String::default(), |tb| {
                tb.format().unwrap_or("failed to parse traceback".to_owned())
            }),
        }
    }
}

/// Python agent initialization result.
#[derive(FromPyObject)]
#[pyo3(from_item_all)]
pub struct InitAgent<'a> {
    /// The actual Python agent.
    pub agent: Option<&'a PyAny>,
    /// An error, if any.
    pub error: Option<PyError>,
}
