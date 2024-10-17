use crate::{
    agents::python::{extract_normalized_iris, extract_normalized_mask},
    utils::RkyvNdarray,
};
use ai_interface::PyError;
use ndarray::Ix2;
use numpy::PyArray2;
use pyo3::{FromPyObject, PyAny, PyResult};
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;

#[derive(FromPyObject)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct PipelineOutput {
    pub error: Option<PyError>,

    pub iris_template: Option<IrisTemplate>,
    pub normalized_image: Option<NormalizedIris>,
    pub normalized_image_resized: Option<NormalizedIris>,
    pub metadata: Metadata,
}

#[derive(FromPyObject, Archive, Serialize, Deserialize, Debug, Clone)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct NormalizedIris {
    #[pyo3(from_py_with = "extract_normalized_iris")]
    pub normalized_image: RkyvNdarray<u8, Ix2>,
    #[pyo3(from_py_with = "extract_normalized_mask")]
    pub normalized_mask: RkyvNdarray<bool, Ix2>,
}

impl NormalizedIris {
    /// Serializes normalized image as a bytes array.
    #[must_use]
    pub fn serialized_image(&self) -> Vec<u8> {
        self.normalized_image
            .as_ndarray()
            .as_standard_layout()
            .as_slice()
            .unwrap()
            .iter()
            .flat_map(|x| x.to_be_bytes())
            .collect()
    }

    /// Serializes normalized mask as a bytes array.
    #[must_use]
    pub fn serialized_mask(&self) -> Vec<u8> {
        self.normalized_mask
            .as_ndarray()
            .as_standard_layout()
            .as_slice()
            .unwrap()
            .iter()
            .flat_map(|x| [u8::from(*x)])
            .collect()
    }

    /// Serializes normalized image and mask as bytes arrays.
    #[must_use]
    pub fn serialized_image_and_mask(&self) -> (Vec<u8>, Vec<u8>) {
        (self.serialized_image(), self.serialized_mask())
    }
}

#[derive(FromPyObject)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct IrisTemplate {
    pub iris_codes: String,
    pub mask_codes: String,
    pub iris_code_version: String,
}

/// Iris metadata.
#[derive(
    FromPyObject, Default, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Metadata {
    pub iris_version: Option<String>,
    pub image_size: Option<(u32, u32)>,
    pub eye_side: Option<String>,
    pub eye_centers: Option<EyeCenters>,
    pub pupil_to_iris_property: Option<PupilToIrisProperty>,
    pub offgaze_score: Option<f64>,
    pub eye_orientation: Option<f64>,
    pub occlusion90: Option<f64>,
    pub occlusion30: Option<f64>,
    pub ellipticity: Option<Ellipticity>,
    pub iris_bbox: Option<BoundingBox>,
    pub template_property: Option<TemplateProperty>,
}

#[derive(
    FromPyObject, Default, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct Ellipticity {
    pupil_ellipticity: Option<f64>,
    iris_ellipticity: Option<f64>,
}
/// Eye centers.
#[derive(
    FromPyObject, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct EyeCenters {
    pupil_center: Option<(f64, f64)>,
    iris_center: Option<(f64, f64)>,
}

/// Pupil-to-iris properties.
#[derive(
    FromPyObject, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct PupilToIrisProperty {
    pupil_to_iris_diameter_ratio: Option<f64>,
    pupil_to_iris_center_dist_ratio: Option<f64>,
}

/// A 2D Bounding Box.
#[derive(
    FromPyObject, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct BoundingBox {
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

#[derive(
    FromPyObject, Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema,
)]
#[pyo3(from_item_all)]
#[allow(missing_docs)]
pub struct TemplateProperty {
    visible_ratio: Option<f64>,
    lower_visible_ratio: Option<f64>,
    upper_visible_ratio: Option<f64>,
    abnormal_mask_ratio: Option<f64>,
    weighted_abnormal_mask_ratio: Option<f64>,
    #[pyo3(from_py_with = "extract_maskcode_hist")]
    #[schemars(with = "Option<Vec<Vec<Vec<u8>>>>")]
    maskcode_hist: Option<Vec<RkyvNdarray<u8, Ix2>>>,
}

fn extract_maskcode_hist(obj: &PyAny) -> PyResult<Option<Vec<RkyvNdarray<u8, Ix2>>>> {
    if obj.is_none() {
        return Ok(None);
    }

    let maskcode_hist: Vec<&PyArray2<u8>> = obj.extract()?;
    Ok(Some(
        maskcode_hist
            .into_iter()
            .map(|py_arr2| RkyvNdarray::from(py_arr2.to_owned_array()))
            .collect(),
    ))
}
