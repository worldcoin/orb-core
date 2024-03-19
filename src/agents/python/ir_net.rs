//! IR-Net python agent.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera,
        camera::Frame,
        python::{check_model_version, choose_config, AgentPython},
        Agent,
    },
    config::Config,
    consts::{IR_HEIGHT, IR_WIDTH},
    inst_elapsed,
    logger::{LogOnError, DATADOG, NO_TAGS},
    port::{Port, SharedPort},
    utils::RkyvNdarray,
};
use eyre::Result;
use ndarray::prelude::*;
use numpy::PyArray2;
use orb_ir_net::IrNet;
use pyo3::prelude::*;
use rkyv::{Archive, Deserialize, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::{collections::HashMap, str, time::Instant};

/// IR-Net python agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct Model {
    configs: Option<HashMap<String, String>>,
}

/// Agent input.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// IR-Net estimate function.
    Estimate {
        /// IR frame.
        frame: camera::ir::Frame,
        /// Physioligical side of eye IR-Net should currently target.
        target_left_eye: bool,
        /// Focus on a matrix of QR codes.
        focus_matrix_code: bool,
    },
    /// Get IR-Net version.
    Version,
    /// Warmup the model with a dummy call.
    Warmup,
}

/// Agent output.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Output {
    /// IR-Net estimate function.
    Estimate(EstimateOutput),
    /// IR-Net version.
    Version(String),
    /// Warmup call response.
    Warmup,
    /// IR-Net returned with a Python exception error.
    Error,
}

/// IR-Net estimate output.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Archive, Serialize, Deserialize)]
pub struct EstimateOutput {
    /// Iris landmarks.
    pub landmarks: Option<RkyvNdarray<f32, Ix2>>,
    /// Fractional sharpness score.
    pub sharpness: f64,
    /// Occlusion 30% score.
    pub occlusion_30: f64,
    /// Occlusion 90% score.
    pub occlusion_90: f64,
    /// Pupil to iris ratio.
    pub pupil_to_iris_ratio: f64,
    /// Offgaze score.
    pub gaze: f64,
    /// Eye-classification score.
    pub eye_detected: f64,
    /// QR-code classification score.
    pub qr_code_detected: f64,
    /// Old occlusion_30 prediction from original output head.
    pub occlusion_30_old: f64,
    /// Indication if eye is opened enough.
    pub eye_opened: bool,
    /// Indication if iris is aligned in frame.
    pub iris_aligned: bool,
    /// Indication if iris is sharp.
    pub iris_sharp: bool,
    /// Indication if iris texture is uncovered.
    pub iris_uncovered: bool,
    /// Validity of eye orientation.
    pub orientation_correct: bool,
    /// Validity of eye gaze.
    pub gaze_valid: bool,
    /// IR Net estimation validity for identification.
    pub valid_for_identification: bool,
    /// IR Net estimation status.
    pub status: i64,
    /// IR-Net status message.
    pub message: String,
    /// Selection score.
    pub score: f64,
    /// Mean brightness of input IR image.
    pub mean_brightness_raw: f64,
    /// Targeted eye.
    pub target_side: u8,
    /// Perceived eye.
    pub perceived_side: Option<i32>,
}

struct Environment<'py> {
    ir_net: IrNet<'py>,
    version: String,
}

impl Port for Model {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Model {
    const SERIALIZED_CONFIG_EXTRA_SIZE: usize = 4096;
    const SERIALIZED_INPUT_SIZE: usize = 4096 + IR_HEIGHT as usize * IR_WIDTH as usize;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096;
}

impl Agent for Model {
    const NAME: &'static str = "ir-net";
}

impl super::Environment<Model> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        let t = Instant::now();

        let (op, res) = match input {
            ArchivedInput::Estimate { frame, target_left_eye, focus_matrix_code } => (
                "estimate",
                self.estimate(py, frame.into_ndarray(), *target_left_eye, *focus_matrix_code)
                    .map(Output::Estimate),
            ),
            ArchivedInput::Version => ("version", Ok(Output::Version(self.version()))),
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
    fn warmup(&self, py: Python) -> Result<()> {
        let image = PyArray2::from_owned_array(py, camera::ir::Frame::default().into_ndarray());
        self.ir_net.estimate(image, false, false)?;
        Ok(())
    }

    fn version(&self) -> String {
        self.version.clone()
    }

    fn estimate(
        &self,
        py: Python,
        frame: Array2<u8>,
        target_left_eye: bool,
        focus_matrix_code: bool,
    ) -> Result<EstimateOutput> {
        let image = PyArray2::from_owned_array(py, frame);
        let estimate = self.ir_net.estimate(image, target_left_eye, focus_matrix_code)?;
        extract(estimate)
    }
}

impl super::AgentPython for Model {
    const DD_NS: &'static str = "ir_net";
    const MINIMUM_MODEL_VERSION: &'static str = "5.0.4";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{} agent: loading model with config: {self:?}", Model::NAME);
        let t = Instant::now();

        let version = check_model_version(IrNet::module(py)?, Model::MINIMUM_MODEL_VERSION)?;
        let config = choose_config(self.configs.as_ref(), &version)?;
        let ir_net = IrNet::init(py, &config)?;

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
        Ok(Box::new(Environment { ir_net, version }))
    }
}

impl From<&Config> for Model {
    fn from(config: &Config) -> Self {
        Self { configs: config.ir_net_model_configs.clone() }
    }
}

/// Extract Rust values from the Python IR Net "estimation" dict
pub fn extract(estimation: &PyAny) -> Result<EstimateOutput> {
    let landmarks = estimation
        .get_item("landmarks")?
        .downcast::<PyArray2<f32>>()
        .ok()
        .map(PyArray2::to_owned_array)
        .map(Into::into);
    let pupil_to_iris_ratio = estimation.get_item("pupil_to_iris_ratio")?.extract()?;
    let occlusion_30 = estimation.get_item("occlusion_30")?.extract()?;
    let occlusion_90 = estimation.get_item("occlusion_90")?.extract()?;
    let sharpness = estimation.get_item("fractional_sharpness_score")?.extract()?;
    let gaze = estimation.get_item("gaze")?.extract()?;
    let eye_detected = estimation.get_item("eye_detected")?.extract()?;
    let qr_code_detected = estimation.get_item("qr_code_detected")?.extract()?;
    let occlusion_30_old = estimation.get_item("occlusion_30_old")?.extract()?;
    let eye_opened = estimation.get_item("eye_opened")?.extract()?;
    let iris_aligned = estimation.get_item("iris_aligned")?.extract()?;
    let iris_sharp = estimation.get_item("iris_sharp")?.extract()?;
    let iris_uncovered = estimation.get_item("iris_uncovered")?.extract()?;
    let orientation_correct = estimation.get_item("orientation_correct")?.extract()?;
    let gaze_valid = estimation.get_item("gaze_valid")?.extract()?;
    let mean_brightness_raw = estimation.get_item("mean_brightness_raw")?.extract()?;
    let valid_for_identification = estimation.get_item("valid_for_identification")?.extract()?;
    let status = estimation.get_item("status")?.extract::<i64>()?;
    let message = estimation.get_item("msg")?.extract::<String>()?;
    let score = calculate_selection_score(sharpness, valid_for_identification, status);
    let target_side = estimation.get_item("target_side")?.extract()?;
    let perceived_side = estimation.get_item("perceived_side")?.extract()?;

    let estimate = EstimateOutput {
        landmarks,
        sharpness,
        occlusion_30,
        occlusion_90,
        pupil_to_iris_ratio,
        gaze,
        eye_detected,
        qr_code_detected,
        occlusion_30_old,
        eye_opened,
        iris_aligned,
        iris_sharp,
        iris_uncovered,
        orientation_correct,
        gaze_valid,
        valid_for_identification,
        status,
        message,
        score,
        mean_brightness_raw,
        target_side,
        perceived_side,
    };

    estimate.log()?;

    Ok(estimate)
}

fn calculate_selection_score(sharpness: f64, valid_for_identification: bool, status: i64) -> f64 {
    if status != 0 || !valid_for_identification || sharpness.is_nan() { -1.0 } else { sharpness }
}

impl EstimateOutput {
    fn log(&self) -> Result<()> {
        tracing::trace!(
            "Ir net result: sharpness {:?}, occlusion_30 {:?}, occlusion_90 {:?}, \
             pupil_to_iris_ratio {:?}, valid_for_identification {:?}, status {:?}, score {:?}, \
             perceived_side {:?}",
            self.sharpness,
            self.occlusion_30,
            self.occlusion_90,
            self.pupil_to_iris_ratio,
            self.valid_for_identification,
            self.status,
            self.score,
            self.perceived_side
        );
        DATADOG.gauge(
            "orb.main.gauge.neural_network.ir_net.sharpness",
            self.sharpness.to_string(),
            NO_TAGS,
        )?;
        DATADOG.gauge(
            "orb.main.gauge.neural_network.ir_net.occlusion_30",
            self.occlusion_30.to_string(),
            NO_TAGS,
        )?;
        DATADOG.gauge(
            "orb.main.gauge.neural_network.ir_net.occlusion_90",
            self.occlusion_90.to_string(),
            NO_TAGS,
        )?;
        DATADOG.gauge(
            "orb.main.gauge.neural_network.ir_net.pupil_to_iris_ratio",
            self.pupil_to_iris_ratio.to_string(),
            NO_TAGS,
        )?;
        if self.valid_for_identification {
            DATADOG.incr(
                "orb.main.count.neural_network.ir_net.valid_for_identification.valid",
                NO_TAGS,
            )?;
        } else {
            DATADOG.incr(
                "orb.main.count.neural_network.ir_net.valid_for_identification.invalid",
                NO_TAGS,
            )?;
        }
        if self.status == 0 {
            DATADOG.incr("orb.main.count.neural_network.ir_net.status.valid", NO_TAGS)?;
        } else {
            DATADOG.incr("orb.main.count.neural_network.ir_net.status.invalid", [format!(
                "status:{}",
                self.status
            )])?;
        }
        DATADOG.gauge(
            "orb.main.gauge.neural_network.ir_net.score",
            self.score.to_string(),
            NO_TAGS,
        )?;
        if let Some(perceived_side) = self.perceived_side {
            DATADOG.gauge(
                "orb.main.gauge.neural_network.ir_net.perceived_side",
                perceived_side.to_string(),
                NO_TAGS,
            )?;
        }
        Ok(())
    }
}

/// Runs the estimate function once in an isolated python context.
pub fn estimate_once(py: Python, frame: &camera::ir::Frame) -> Result<EstimateOutput> {
    let ir_net = IrNet::init(py, &String::new())?;
    let shape = (frame.height() as usize, frame.width() as usize);
    let image = Array::from_shape_vec(shape, frame.to_vec())?;
    let image = PyArray2::from_owned_array(py, image);
    let estimate = ir_net.estimate(image, false, false)?;
    extract(estimate)
}
