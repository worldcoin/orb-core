//! Occlusion detection python agent.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{
        camera,
        python::{
            check_model_version,
            rgb_net::{ArchivedRectangle, Rectangle},
            AgentPython,
        },
        ProcessInitializer,
    },
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
    dd_timing,
};
use agentwire::{
    agent::{self, Agent as _},
    port::{self, Port, SharedPort},
};
use ai_interface::PyError;
use eyre::{Error, Result};
use ndarray::prelude::*;
use numpy::{PyArray1, PyArray3};
use pyo3::{prelude::*, types::PyDict};
use rkyv::{Archive, Deserialize, Infallible, Serialize};
use schemars::JsonSchema;
use serde::Serialize as SerdeSerialize;
use std::{mem::size_of, time::Instant};

/// Occlusion detection python agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Default, Clone, Debug, Archive, Serialize, Deserialize, SerdeSerialize, JsonSchema)]
pub struct Model {}

/// Agent input.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// Occlusion detection estimate function.
    Estimate {
        /// RGB frame.
        frame: camera::rgb::Frame,
        /// Face bounding box.
        bbox: Rectangle,
    },
}

/// Agent output.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub enum Output {
    /// Occlusion detection estimate function.
    Estimate(EstimateOutput),
    /// Occlusion detection returned with a Python exception error.
    Error(PyError),
}

/// Occlusion detection estimate output.
#[derive(Archive, Clone, Debug, Default, Deserialize, Serialize)]
pub struct EstimateOutput {
    /// Occlusion assert.
    pub occlusion: bool,
    /// Eye glasses probability score.
    pub eye_glasses_probability: f64,
    /// Eye glasses occlusion
    pub eye_glasses_occlusion: bool,
    /// Face mask probability score.
    pub face_mask_probability: f64,
    /// Face mask occlusion
    pub face_mask_occlusion: bool,
    /// Face bounding box.
    pub bbox: Rectangle,
}

struct Environment<'py> {
    agent: &'py PyAny,
}

impl Port for Model {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Model {
    const SERIALIZED_INIT_SIZE: usize =
        size_of::<usize>() + size_of::<<Model as Archive>::Archived>();
    const SERIALIZED_INPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096;
}

impl agentwire::Agent for Model {
    const NAME: &'static str = "occlusion-detection";
}

impl super::Environment<Model> for Environment<'_> {
    fn iterate(&mut self, py: Python, input: &ArchivedInput) -> Result<Output> {
        let t = Instant::now();

        let (op, res) = match input {
            ArchivedInput::Estimate { frame, bbox } => {
                ("estimate", self.occlusion_estimate(py, frame, bbox).map(Output::Estimate))
            }
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
            } else {
                Err(e)
            }
        })
    }
}

impl Environment<'_> {
    fn occlusion_estimate(
        &mut self,
        py: Python,
        frame: &camera::rgb::ArchivedFrame,
        bbox: &ArchivedRectangle,
    ) -> Result<EstimateOutput> {
        let kwargs = PyDict::new(py);
        kwargs.set_item("margin", 10)?;
        kwargs.set_item(
            "bbox",
            PyArray1::from_owned_array(
                py,
                Array::from_shape_vec((4,), vec![
                    bbox.start_x,
                    bbox.start_y,
                    bbox.end_x,
                    bbox.end_y,
                ])
                .unwrap(),
            ),
        )?;
        let image = PyArray3::from_owned_array(py, frame.into_ndarray());

        let estimation = self.agent.call_method("estimate", (image,), Some(kwargs))?;

        // Get results.
        let occlusion = estimation.get_item("occlusion")?.extract()?;
        let eye_glasses_probability =
            estimation.get_item("eye-glasses")?.get_item("glasses")?.extract()?;
        let eye_glasses_occlusion =
            estimation.get_item("eye-glasses")?.get_item("occlusion")?.extract()?;

        let face_mask_probability =
            estimation.get_item("face-mask")?.get_item("mask")?.extract()?;
        let face_mask_occlusion =
            estimation.get_item("face-mask")?.get_item("occlusion")?.extract()?;
        Ok(EstimateOutput {
            occlusion,
            eye_glasses_probability,
            eye_glasses_occlusion,
            face_mask_probability,
            face_mask_occlusion,
            bbox: bbox.deserialize(&mut Infallible).unwrap(),
        })
    }
}

impl super::AgentPython for Model {
    const DD_NS: &'static str = "occlusion";
    // TODO: misreported 1.0.2 as 1.0.0.
    const MINIMUM_MODEL_VERSION: &'static str = "1.0.0";

    fn init<'py>(self, py: Python<'py>) -> Result<Box<dyn super::Environment<Self> + 'py>> {
        tracing::info!("{} agent: loading model with config: {self:?}", Model::NAME);
        let t = Instant::now();

        let module = py.import("occlusion_detection")?;
        check_model_version(module, Model::MINIMUM_MODEL_VERSION)?;
        let agent = module.getattr("Occlusion")?.call0()?;

        tracing::info!(
            "Python agent {} <benchmark>: initialization done in {} ms",
            Model::NAME,
            t.elapsed().as_millis()
        );
        dd_timing!("main.time.neural_network.init" + format!("{}", Model::DD_NS), t);
        Ok(Box::new(Environment { agent }))
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
