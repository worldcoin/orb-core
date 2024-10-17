use crate::{
    agents::python::{extract_rkyv_ndarray_d1, rgb_net},
    utils::RkyvNdarray,
};
use ai_interface::PyError;
use ndarray::{Ix1, Ix3};
use numpy::PyArray3;
use pyo3::prelude::*;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use std::collections::HashMap;

#[derive(FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Bundle {
    pub error: Option<PyError>,

    pub thumbnail: Option<Thumbnail>,
    pub embeddings: Option<Vec<Embedding>>,
    pub inference_backend: Option<String>,
}

#[derive(FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Embedding {
    #[pyo3(from_py_with = "extract_rkyv_ndarray_d1")]
    pub embedding: RkyvNdarray<u32, Ix1>,
    pub embedding_type: String,
    pub embedding_version: String,
    pub embedding_inference_backend: String,
}

#[derive(FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Thumbnail {
    pub border: Option<(f64, f64, f64, f64)>,
    pub bounding_box: Option<BBox>,
    #[pyo3(from_py_with = "extract_option_rkyv_ndarray_d3")]
    pub image: Option<RkyvNdarray<u8, Ix3>>,
    pub rotated_angle: Option<f64>,
    pub shape: Option<(u64, u64, u64)>,
    pub original_shape: Option<(u64, u64, u64)>,
    pub original_image: Option<String>,
}

fn extract_option_rkyv_ndarray_d3(obj: &PyAny) -> PyResult<Option<RkyvNdarray<u8, Ix3>>> {
    if obj.is_none() {
        return Ok(None);
    }
    let arr: &PyArray3<u8> = obj.extract()?;
    Ok(Some(RkyvNdarray::from(arr.to_owned_array())))
}

#[derive(
    FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct BBox {
    pub origin: Point,
    pub height: f64,
    pub rotation: f64,
    pub width: f64,
}

#[derive(
    FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(
    FromPyObject, Debug, Default, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct FraudChecks {
    pub error: Option<PyError>,
}

#[derive(
    FromPyObject, Debug, Default, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Triplet<T> {
    pub left: T,
    pub right: T,
    pub self_custody: T,
}

#[derive(
    FromPyObject, Debug, Default, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct FIIsValidOutput {
    pub error: Option<PyError>,
    pub inference_backend: Option<String>,

    pub is_valid: Option<bool>,
    pub score: Option<f64>,
}

#[derive(Debug, Default, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
#[allow(missing_docs)]
pub struct IsValidOutput {
    pub error: Option<PyError>,
    pub inference_backend: Option<String>,

    pub is_valid: Option<bool>,
    pub score: Option<f64>,

    // This is a hack. The following fields are not expected to come from the FI model. We manually populate them in
    // Orb-Core.
    pub rgb_net_eye_landmarks: (rgb_net::Point, rgb_net::Point),
    pub rgb_net_bbox: rgb_net::Rectangle,
}

#[derive(Debug, Default, Clone, Archive, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct ValidationsOutput {
    pub quality: bool,
    pub face_detected: bool,
}

#[derive(
    FromPyObject, Debug, Clone, Default, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct BoundingBox {
    pub height: f64,
    pub origin: Point,
    pub rotation: f64,
    pub width: f64,
}

/// Convenience wrapper struct for the Face Identifier model's configuration coming from the backend.
#[derive(
    Archive, Serialize, Deserialize, SerdeDeserialize, SerdeSerialize, Debug, Clone, JsonSchema,
)]
#[serde(rename_all = "PascalCase")]
pub struct BackendConfig {
    /// Face Identifier: Namespaced model configs in base64.
    pub face_identifier_model_configs: Option<HashMap<String, String>>,
}
