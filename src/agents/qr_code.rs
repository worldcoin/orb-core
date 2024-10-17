//! QR-code reader from the RGB camera.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::{
    agents::{camera::rgb::Frame, ProcessInitializer},
    consts::{RGB_NATIVE_HEIGHT, RGB_NATIVE_WIDTH},
};
use agentwire::{
    agent,
    port::{Port, RemoteInner, SharedPort},
};
use eyre::{Error, Result};
use image::{DynamicImage, RgbImage};
use rkyv::{Archive, Deserialize, Serialize};
use rxing::{
    common::HybridBinarizer, qrcode::cpp_port::QrReader, BinaryBitmap,
    BufferedImageLuminanceSource, Reader,
};
use std::mem::size_of;

/// QR-code reader from the RGB camera.
///
/// See [the module-level documentation](self) for details.
#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Agent {}

/// Qr-code reader output.
#[derive(Debug, Archive, Serialize, Deserialize)]
pub struct Output {
    /// Detected QR-code value.
    pub payload: String,
    /// QR-code corner coordinates.
    pub points: Points,
}

/// Qr-code reader input.
#[derive(Debug, Archive, Serialize)]
pub enum Input {
    /// RGB camera frame.
    Frame(Frame),
    /// Ambient light sensor value.
    Als(u32),
}

/// QR-code corner coordinates.
pub type Points = Vec<(f32, f32)>;

impl Port for Agent {
    type Input = Input;
    type Output = Output;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl SharedPort for Agent {
    const SERIALIZED_INIT_SIZE: usize =
        size_of::<usize>() + size_of::<<Agent as Archive>::Archived>();
    const SERIALIZED_INPUT_SIZE: usize =
        4096 + RGB_NATIVE_HEIGHT as usize * RGB_NATIVE_WIDTH as usize * 3;
    const SERIALIZED_OUTPUT_SIZE: usize = 4096;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "qr-code";
}

impl agentwire::agent::Process for Agent {
    type Error = Error;

    fn run(self, mut port: RemoteInner<Self>) -> Result<(), Self::Error> {
        let mut qr_scanner = QrReader;
        loop {
            let input = port.recv();
            match input.value {
                ArchivedInput::Frame(frame) => {
                    match decode_rxing(
                        &mut qr_scanner,
                        frame.data().to_vec(),
                        frame.width(),
                        frame.height(),
                    ) {
                        Ok(output) => {
                            tracing::debug!("Decoded QR-code with rxing: {:?}", output.payload);
                            let chain = input.chain_fn();
                            port.try_send(&chain(output));
                        }
                        Err(e) => {
                            if !matches!(e, rxing::Exceptions::NotFoundException(_)) {
                                tracing::debug!("rxing error: {}", e);
                            }
                        }
                    }
                }
                ArchivedInput::Als(_) => {}
            }
        }
    }

    fn exit_strategy(_code: Option<i32>, _signal: Option<i32>) -> agent::process::ExitStrategy {
        // Because crashes are deterministic for this agent, we will not retry
        // bad inputs.
        agent::process::ExitStrategy::Restart
    }

    fn initializer() -> impl agent::process::Initializer {
        ProcessInitializer::default()
    }
}

#[allow(clippy::cast_precision_loss)]
fn decode_rxing(
    qr_scanner: &mut QrReader,
    image: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<Output, rxing::Exceptions> {
    let mut binarized_image = BinaryBitmap::new(HybridBinarizer::new(
        BufferedImageLuminanceSource::new(DynamicImage::ImageRgb8(
            RgbImage::from_vec(width, height, image)
                .expect("image size to be at least 3*width*height"),
        )),
    ));
    let rxing_result = qr_scanner.decode(&mut binarized_image)?;
    Ok(Output {
        payload: rxing_result.getText().to_owned(),
        points: rxing_result
            .getPoints()
            .iter()
            .map(|p| (p.x / width as f32, p.y / height as f32))
            .collect(),
    })
}
