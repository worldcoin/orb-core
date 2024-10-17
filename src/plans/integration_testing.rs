//! Collection of tools used in integration testing.

use crate::agents::{
    camera,
    camera::{ir, Frame},
    python,
};

use crate::plans::biometric_capture;

use eyre::Result;
use orb_ir_net::IrNet;
use pyo3::Python;

use std::{
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
    time::SystemTime,
};

/// Configuration for hacks used for integration testing
#[derive(Default, Debug, Clone)]
pub struct CiHacks {
    /// Path to the PNG phone of the left eyes to inject in the pipeline
    pub left_eye: Option<PathBuf>,
    /// Path to the PNG phone of the right eyes to inject in the pipeline
    pub right_eye: Option<PathBuf>,
}

impl FromStr for CiHacks {
    type Err = eyre::Error;

    /// Split the str in two comma separated strings
    fn from_str(s: &str) -> Result<Self> {
        let mut hacks = Self::default();
        let mut parts = s.split(',');
        hacks.left_eye = parts.next().map(PathBuf::from);
        hacks.right_eye = parts.next().map(PathBuf::from);
        if parts.next().is_some() {
            return Err(eyre::eyre!("expected two comma separated paths, got {}", s));
        }
        Ok(hacks)
    }
}

fn decode_png(image_path: &Path) -> (png::OutputInfo, Vec<u8>) {
    let image_file = File::open(image_path).unwrap();
    let decoder = png::Decoder::new(image_file);
    let (info, mut reader) = decoder.read_info().unwrap();
    let mut buf = vec![0; info.buffer_size()];
    reader.next_frame(&mut buf).unwrap();
    (info, buf)
}

fn load_ir_frame(eye_image_path: &Path) -> Result<ir::Frame> {
    let (info, buf) = decode_png(eye_image_path);
    let frame = camera::ir::Frame::new(
        buf,
        SystemTime::UNIX_EPOCH.elapsed()?,
        info.width,
        info.height,
        0, // mean value is not used in the calculation pipeline
    );
    Ok(frame)
}

fn run_ir_net_estimate(frame: &ir::Frame) -> Result<python::ir_net::EstimateOutput> {
    Python::with_gil(|py| {
        python::init_sys_argv(py);
        let buf = &frame.to_vec();
        let ir_net = IrNet::init(py, &String::new()).unwrap();
        let shape = (frame.height() as usize, frame.width() as usize);
        let image = ndarray::Array::from_shape_vec(shape, buf.clone()).unwrap();
        let image = numpy::PyArray2::from_owned_array(py, image);
        let ir_net_estimate = python::ir_net::extract(ir_net.estimate(image, false, false)?)?;
        Ok(ir_net_estimate)
    })
}

impl CiHacks {
    /// Take the captured eye's image and replace them with content of PNG file if provided.
    pub fn replace_captured_eyes(
        &self,
        capture: &mut Option<biometric_capture::Capture>,
    ) -> Result<()> {
        if let Some(cap) = capture {
            if let Some(ref left_eye_path) = self.left_eye {
                tracing::info!(
                    "Replacing the left eye IR images with {} image",
                    left_eye_path.display()
                );
                cap.eye_left.ir_frame = load_ir_frame(left_eye_path)?;
                cap.eye_left.ir_net_estimate = run_ir_net_estimate(&cap.eye_left.ir_frame)?;
            }
            if let Some(ref right_eye_path) = self.right_eye {
                tracing::info!(
                    "Replacing the right eye IR images with {} image",
                    right_eye_path.display()
                );
                cap.eye_right.ir_frame = load_ir_frame(right_eye_path)?;
                cap.eye_right.ir_net_estimate = run_ir_net_estimate(&cap.eye_right.ir_frame)?;
            }
        }
        Ok(())
    }
}
