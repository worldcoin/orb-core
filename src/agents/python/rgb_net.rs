//! RGB-Net python agent.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera::{self, Frame},
        python::{check_model_version, AgentPython},
        Agent,
    },
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
    get_and_extract, get_item, inst_elapsed,
    logger::{LogOnError, DATADOG, NO_TAGS},
    port::{Port, SharedPort},
};
use eyre::{Result, WrapErr};
use ndarray::prelude::*;
use numpy::PyArray3;
use orb_rgb_net::RgbNet;
use pyo3::prelude::*;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::time::Instant;

/// RGB-Net python agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct Model {}

/// Agent input.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// RGB-Net estimate function.
    Estimate {
        /// RGB frame.
        frame: camera::rgb::Frame,
    },
    /// Warmup the model with a dummy call.
    Warmup,
}

/// Agent output.
#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub enum Output {
    /// RGB-Net estimate function.
    Estimate(EstimateOutput),
    /// RGB-Net init_undistort function.
    InitUndistort,
    /// Warmup call response.
    Warmup,
    /// RGB-Net returned with a Python exception error.
    Error,
}

/// RGB-Net estimate output.
#[derive(Clone, Debug, Default, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct EstimateOutput {
    /// Current version number.
    pub rgbnet_version: String,
    /// RGB-Net predictions.
    pub predictions: Vec<EstimatePredictionOutput>,
}

/// RGB-Net estimation for a person in frame.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct EstimatePredictionOutput {
    /// Bounding box prediction.
    pub bbox: EstimatePredictionBboxOutput,
    /// Landmarks prediction.
    pub landmarks: EstimatePredictionLandmarksOutput,
}

/// RGB-Net bounding box prediction for a person.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct EstimatePredictionBboxOutput {
    /// Bounding box coordinates.
    pub coordinates: Rectangle,
    /// Whether the prediction is the primary prediction in the prediction set.
    pub is_primary: bool,
    /// Prediction score.
    pub score: f64,
}

/// RGB-Net landmarks prediction for a person.
#[derive(Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct EstimatePredictionLandmarksOutput {
    /// Left eye coordinates.
    pub left_eye: Point,
    /// Left mouth corner coordinates.
    pub left_mouth: Point,
    /// Nose coordinates.
    pub nose: Point,
    /// Right eye coordinates.
    pub right_eye: Point,
    /// Right mouth corner coordinates.
    pub right_mouth: Point,
}

/// RGB-Net rectangle.
#[derive(
    FromPyObject,
    Default,
    Clone,
    Copy,
    Debug,
    Archive,
    Serialize,
    Deserialize,
    SerdeSerialize,
    JsonSchema,
)]
pub struct Rectangle {
    /// Start coordinate for x.
    pub start_x: f64,
    /// Start coordinate for y.
    pub start_y: f64,
    /// End coordinate for x.
    pub end_x: f64,
    /// End coordinate for y.
    pub end_y: f64,
}

/// RGB-Net point.
#[derive(
    FromPyObject,
    Default,
    Clone,
    Copy,
    Debug,
    Archive,
    Serialize,
    Deserialize,
    SerdeSerialize,
    JsonSchema,
)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

/// Environment wrapper for the python agent.
pub struct Environment<'py> {
    pub(super) rgb_net: RgbNet<'py>,
}

impl Port for Model {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Model {
    const SERIALIZED_CONFIG_EXTRA_SIZE: usize = 0;
    const SERIALIZED_INPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096;
}

impl super::Agent for Model {
    const NAME: &'static str = "rgb-net";
}

impl super::Environment<Model> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        let t = Instant::now();

        let (op, res) = match input {
            ArchivedInput::Estimate { frame } => {
                ("estimate", self.rgb_net_estimate(py, frame.into_ndarray()).map(Output::Estimate))
            }
            ArchivedInput::Warmup => ("warmup", self.warmup(py).map(|()| Output::Warmup)),
        };

        DATADOG
            .timing(
                format!("orb.main.time.processing.{}.{}", Model::DD_NS, op),
                inst_elapsed!(t),
                NO_TAGS,
            )
            .or_log();
        tracing::trace!(
            "Python agent {}::{} <benchmark>: {} ms",
            Model::NAME,
            op,
            inst_elapsed!(t)
        );

        res.or_else(|e| {
            if let Some(pe) = e.downcast_ref::<PyErr>() {
                <Model as super::AgentPython>::report_python_exception(py, &e, pe);
                Ok(Output::Error)
            } else {
                Err(e)
            }
        })
    }
}

impl Environment<'_> {
    /// Create a new python agent environment.
    pub fn new(py: Python<'_>) -> Result<Environment<'_>> {
        let t = Instant::now();

        let rgb_net = RgbNet::init(py)?;
        check_model_version(rgb_net.module(), Model::MINIMUM_MODEL_VERSION)?;

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

        Ok(Environment { rgb_net })
    }

    /// Run the estimate function.
    pub fn rgb_net_estimate(&self, py: Python, frame: Array3<u8>) -> Result<EstimateOutput> {
        let estimation = self.rgb_net.estimate(PyArray3::from_owned_array(py, frame))?;
        extract(estimation).wrap_err("RGB-Net rgb_net_estimate.extract failed")
    }

    /// Run the estimate function.
    pub fn warmup(&self, py: Python) -> Result<()> {
        self.rgb_net.estimate(PyArray3::from_owned_array(
            py,
            camera::rgb::Frame::default().into_ndarray(),
        ))?;
        Ok(())
    }
}

impl super::AgentPython for Model {
    const DD_NS: &'static str = "rgb_net";
    const MINIMUM_MODEL_VERSION: &'static str = "2.0.2";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{} agent: loading model with config: {self:?}", Model::NAME);
        Ok(Box::new(Environment::new(py)?))
    }
}

impl EstimateOutput {
    /// Returns the primary prediction.
    #[must_use]
    pub fn primary(&self) -> Option<&EstimatePredictionOutput> {
        self.predictions.iter().find(|prediction| prediction.bbox.is_primary)
    }
}

impl EstimatePredictionOutput {
    /// Estimates user distance.
    #[must_use]
    pub fn user_distance(&self) -> f64 {
        const ALPHA_RGB_CAMERA: f64 = 40.0;
        let delta_x = (self.landmarks.left_eye.x - self.landmarks.right_eye.x).abs();
        let delta_y = (self.landmarks.left_eye.y - self.landmarks.right_eye.y).abs();
        let iris_distance_in_percent = (delta_x.powi(2) + delta_y.powi(2)).sqrt();
        ALPHA_RGB_CAMERA / iris_distance_in_percent
    }

    /// Returns `true` if both eyes are detected.
    #[must_use]
    pub fn is_face_detected(&self) -> bool {
        !self.landmarks.left_eye.x.is_nan()
            && !self.landmarks.left_eye.y.is_nan()
            && !self.landmarks.right_eye.x.is_nan()
            && !self.landmarks.right_eye.y.is_nan()
    }
}

impl Rectangle {
    /// Returns `true` if the coordinates fall in the `[0.0; 1.0]` range.
    #[must_use]
    pub fn is_correct(&self) -> bool {
        self.start_x >= 0.0 && self.end_x <= 1.0 && self.start_y >= 0.0 && self.end_y <= 1.0
    }
}

/// Extract Rust values from the Python RGB Net "estimation" dict
pub fn extract(estimation: &PyAny) -> Result<EstimateOutput> {
    let rgbnet_version = get_and_extract!(estimation, "rgbnet_version")?;
    let rgbnet_predictions = get_item!(estimation, "predictions")?;
    let rgbnet_predictions_len =
        rgbnet_predictions.len().wrap_err("failed to .len() 'predictions'")?;
    let mut predictions = Vec::with_capacity(rgbnet_predictions_len);
    for i in 0..rgbnet_predictions_len {
        let rgbnet_prediction = get_item!(rgbnet_predictions, &i)?;
        let rgbnet_bbox = get_item!(rgbnet_prediction, "bbox")?;
        let rgbnet_landmarks = get_item!(rgbnet_prediction, "landmarks")?;
        predictions.push(EstimatePredictionOutput {
            bbox: EstimatePredictionBboxOutput {
                coordinates: extract_rectangle(get_item!(rgbnet_bbox, "coordinates")?)?,
                is_primary: get_and_extract!(rgbnet_bbox, "is_primary")?,
                score: get_and_extract!(rgbnet_bbox, "score")?,
            },
            landmarks: EstimatePredictionLandmarksOutput {
                left_eye: extract_point(get_item!(rgbnet_landmarks, "left_eye")?)?,
                left_mouth: extract_point(get_item!(rgbnet_landmarks, "left_mouth")?)?,
                nose: extract_point(get_item!(rgbnet_landmarks, "nose")?)?,
                right_eye: extract_point(get_item!(rgbnet_landmarks, "right_eye")?)?,
                right_mouth: extract_point(get_item!(rgbnet_landmarks, "right_mouth")?)?,
            },
        });
    }
    Ok(EstimateOutput { rgbnet_version, predictions })
}

fn extract_rectangle(coordinates: &PyAny) -> Result<Rectangle> {
    let start_x = get_and_extract!(coordinates, &0)?;
    let start_y = get_and_extract!(coordinates, &1)?;
    let end_x = get_and_extract!(coordinates, &2)?;
    let end_y = get_and_extract!(coordinates, &3)?;
    Ok(Rectangle { start_x, start_y, end_x, end_y })
}

fn extract_point(coordinates: &PyAny) -> Result<Point> {
    let x = get_and_extract!(coordinates, &0)?;
    let y = get_and_extract!(coordinates, &1)?;
    Ok(Point { x, y })
}

/// Runs the estimate funciton once in an isolated python context.
pub fn estimate_once(py: Python, frame: &camera::rgb::Frame) -> Result<EstimateOutput> {
    let rgb_net = RgbNet::init(py)?;
    let shape = (frame.height() as usize, frame.width() as usize, 3);
    let image = Array::from_shape_vec(shape, frame.to_vec())?;
    let image = PyArray3::from_owned_array(py, image);
    let estimate = rgb_net.estimate(image)?;
    extract(estimate)
}
