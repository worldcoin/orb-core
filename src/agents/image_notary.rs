//! Notarizes images and assigns them image IDs.
//!
//! IF AND ONLY IF the `internal-data-acquisition` feature is enabled,
//! it also saves files to disk
#![cfg_attr(
    feature = "internal-data-acquisition",
    doc = "which will eventually be uploaded by [`crate::agents::image_uploader`]."
)]

#[cfg(all(
    not(feature = "no-image-encryption"),
    any(feature = "internal-data-acquisition", test)
))]
use crate::agents::encrypt_and_seal;
use crate::{
    agents::{
        camera::{self, Frame, FrameResolution},
        python::{self, rgb_net},
    },
    consts::{
        DATA_ACQUISITION_BASE_DIR, MIN_AVAILABLE_SSD_SPACE, MIN_AVAILABLE_SSD_SPACE_BEFORE_SIGNUP,
    },
    dd_incr,
    debug_report::{FaceIdentifierIsValidMetadata, IrNetMetadata},
    mcu::main::IrLed,
    plans::biometric_capture::{EyeCapture, SelfCustodyCandidate},
    ssd,
    time_series::TimeSeries,
};
use agentwire::port::{self, Port};
use eyre::{bail, Error, Result, WrapErr};
use futures::{channel::oneshot, prelude::*};
use orb_wld_data_id::{ImageId, SignupId};
use ordered_float::OrderedFloat;
use png::EncodingError;
#[cfg(not(test))]
use std::convert::Infallible;
use std::{
    clone::Clone,
    collections::{BTreeMap, HashMap},
    convert::identity,
    fs,
    mem::take,
    path::{Path, PathBuf},
    time::Duration,
};
use time::{format_description::well_known::Rfc2822, OffsetDateTime};
use tokio::runtime;
use walkdir::WalkDir;

type SharpnessHeaps = HashMap<
    (IrLed, bool),
    BTreeMap<OrderedFloat<f64>, (Option<python::ir_net::EstimateOutput>, camera::ir::Frame)>,
>;

#[cfg(not(feature = "internal-data-acquisition"))]
use std::fs::remove_dir_all;

#[cfg(feature = "internal-data-acquisition")]
use crate::{
    consts::{
        IRIS_SCORE_MIN, IR_EYE_SAVE_FPS, IR_FACE_SAVE_FPS, NUM_SHARP_IR_FRAMES, RGB_SAVE_FPS,
        THERMAL_SAVE_FPS,
    },
    utils::sample_at_fps,
};

#[allow(missing_docs)]
#[derive(Default, Debug)]
pub struct Agent {
    signup_id: SignupId,
    save_dir: PathBuf,
    last_ir_save_time: Duration,
    last_ir_face_save_time: Duration,
    last_rgb_save_time: Duration,
    last_thermal_save_time: Duration,
    sharpest_frames: SharpnessHeaps,
    log: Log,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub enum Input {
    /// Sets the SignupId as save directory and resets internal state.
    InitializeSignup {
        signup_id: SignupId,
    },
    /// Persist the face/eye images associated with identification (for debugging)
    SaveIdentificationImages(Box<SaveIdentificationImagesInput>),
    SaveIrNetEstimate(SaveIrNetEstimateInput),
    SaveIrFaceData(SaveIrFaceDataInput),
    SaveRgbNetEstimate(SaveRgbNetEstimateInput),
    SaveFusionRnFi(SaveFusionRnFiInput),
    SaveThermalData(SaveThermalDataInput),
    /// Get the sharpest frame since the initialization
    GetSharpestFrame(GetSharpestFrameInput),
    /// Write the sharpest frames seen since initialization to disk
    FinalizeSignup,
    /// Takes the configuration log from the agent.
    TakeLog(oneshot::Sender<Log>),
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveIdentificationImagesInput {
    pub tx: oneshot::Sender<Option<IdentificationImages>>,
    pub left: EyeCapture,
    pub right: EyeCapture,
    pub self_custody_candidate: SelfCustodyCandidate,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveIrNetEstimateInput {
    pub estimate: Option<python::ir_net::EstimateOutput>,
    pub frame: camera::ir::Frame,
    pub wavelength: IrLed,
    pub target_left_eye: bool,
    /// If not `None`, overrides the target FPS for saving.
    pub fps_override: Option<f32>,
    pub log_metadata_always: bool,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveIrFaceDataInput {
    pub frame: camera::ir::Frame,
    pub wavelength: IrLed,
    /// If not `None`, overrides the target FPS for saving.
    pub fps_override: Option<f32>,
    pub log_metadata_always: bool,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveRgbNetEstimateInput {
    pub estimate: python::rgb_net::EstimateOutput,
    pub frame: camera::rgb::Frame,
    pub log_metadata_always: bool,
    pub resolution_override: Option<FrameResolution>,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveFusionRnFiInput {
    pub estimate: python::rgb_net::EstimateOutput,
    pub is_valid: python::face_identifier::types::IsValidOutput,
    pub frame: camera::rgb::Frame,
    pub log_metadata_always: bool,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct SaveThermalDataInput {
    pub frame: camera::thermal::Frame,
    pub wavelength: IrLed,
    /// If not `None`, overrides the target FPS for saving.
    pub fps_override: Option<f32>,
    pub log_metadata_always: bool,
}

#[allow(missing_docs)]
#[derive(Debug)]
pub struct GetSharpestFrameInput {
    pub wavelength: IrLed,
    pub side: bool,
    pub tx: oneshot::Sender<Option<python::ir_net::EstimateOutput>>,
}

#[cfg(test)]
#[allow(missing_docs)]
#[derive(Debug)]
pub enum Output {
    /// The path the IR eye frame is saved to
    IrEyeFramePath(PathBuf),
    /// The path the IR face frame is saved to
    IrFaceFramePath(PathBuf),
    /// The path the thermal frame is saved to
    ThermalFramePath(PathBuf),
}

impl Port for Agent {
    type Input = Input;
    #[cfg(not(test))]
    type Output = Infallible;
    #[cfg(test)]
    type Output = Output;

    const INPUT_CAPACITY: usize = 10;
    const OUTPUT_CAPACITY: usize = 10;
}

/// History of metadata received by this agent.
/// These values include image_ids, which are assigned within this agent.
#[derive(Debug)]
pub struct Log {
    /// IR Net metadata history
    #[allow(clippy::type_complexity)]
    pub ir_net_metadata: TimeSeries<(ImageId, Option<IrNetMetadata>, IrLed, bool, Duration, bool)>,
    /// IR Face metadata
    pub ir_face_metadata: TimeSeries<(ImageId, IrLed, Duration, bool)>,
    /// RGB Net metadata history
    pub rgb_net_metadata: TimeSeries<(ImageId, rgb_net::EstimateOutput, Duration, bool)>,
    /// Face Identifier metadata history
    pub face_identifier_metadata:
        TimeSeries<(ImageId, FaceIdentifierIsValidMetadata, Duration, bool)>,
    /// Thermal metadata
    pub thermal_metadata: TimeSeries<(ImageId, IrLed, Duration, bool)>,
}

#[derive(Clone, Default, Debug)]
#[allow(missing_docs)]
pub struct IdentificationImages {
    pub left_ir: ImageId,
    pub left_ir_940nm: Option<ImageId>,
    pub left_ir_740nm: Option<ImageId>,
    pub right_ir: ImageId,
    pub right_ir_940nm: Option<ImageId>,
    pub right_ir_740nm: Option<ImageId>,
    pub left_rgb: ImageId,
    pub left_rgb_fullres: ImageId,
    pub right_rgb: ImageId,
    pub right_rgb_fullres: ImageId,
    pub self_custody_candidate: ImageId,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            ir_net_metadata: TimeSeries::builder().build(),
            ir_face_metadata: TimeSeries::builder().build(),
            rgb_net_metadata: TimeSeries::builder().build(),
            face_identifier_metadata: TimeSeries::builder().build(),
            thermal_metadata: TimeSeries::builder().build(),
        }
    }
}

/// Gets the sharpest frame seen since the last `InitializeSignup` message
/// was received.
pub async fn get_sharpest_frame(
    port: &mut port::Outer<Agent>,
    wavelength: IrLed,
    target_left_eye: bool,
) -> Result<Option<python::ir_net::EstimateOutput>> {
    let (tx, rx) = oneshot::channel();
    port.send(port::Input::new(Input::GetSharpestFrame(GetSharpestFrameInput {
        wavelength,
        side: target_left_eye,
        tx,
    })))
    .await?;
    Ok(rx.await?)
}

/// Saves eye / face identification images and returns their IDs.
pub async fn save_identification_images(
    port: &mut port::Outer<Agent>,
    left: EyeCapture,
    right: EyeCapture,
    self_custody_candidate: SelfCustodyCandidate,
) -> Result<Option<IdentificationImages>> {
    let (tx, rx) = oneshot::channel();
    port.send(port::Input::new(Input::SaveIdentificationImages(Box::new(
        SaveIdentificationImagesInput { tx, left, right, self_custody_candidate },
    ))))
    .await?;
    Ok(rx.await?)
}

/// Takes the configuration history since the last `InitializeSignup`
/// message was received.
pub async fn take_log(port: &mut port::Outer<Agent>) -> Result<Log> {
    let (tx, rx) = oneshot::channel();
    port.send(port::Input::new(Input::TakeLog(tx))).await?;
    Ok(rx.await?)
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "image-saver";
}

impl agentwire::agent::Thread for Agent {
    type Error = Error;

    fn run(mut self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        'signup: while let Some(input) = rt.block_on(port.next()) {
            self.signup_id = match input.value {
                Input::InitializeSignup { signup_id } => signup_id,
                input => bail!("Unexpected image_notary input: {input:?}"),
            };
            ensure_enough_space().wrap_err("auto deletion")?;
            tracing::debug!(
                "There is {} bytes available on the SSD before signup",
                ssd::available_space()
            );
            self.initialize_signup();
            while let Some(input) = rt.block_on(port.next()) {
                match input.value {
                    // These saved images are currently unused, but helpful for debugging
                    Input::SaveIdentificationImages(input) => {
                        self.handle_save_identification_images(*input)?;
                    }
                    Input::SaveIrNetEstimate(input) => {
                        self.handle_save_ir_net_estimate(input, &mut port)?;
                    }
                    Input::SaveIrFaceData(input) => {
                        self.handle_save_ir_face_data(input, &mut port)?;
                    }
                    Input::SaveRgbNetEstimate(input) => {
                        self.handle_save_rgb_net_estimate(input)?;
                    }
                    Input::SaveFusionRnFi(input) => {
                        self.handle_save_fusion_rn_fi(input)?;
                    }
                    Input::SaveThermalData(input) => {
                        self.handle_save_thermal_data(input, &mut port)?;
                    }
                    Input::GetSharpestFrame(input) => {
                        self.handle_get_sharpest_frame(input);
                    }
                    Input::FinalizeSignup => {
                        self.finalize_signup()?;
                    }
                    Input::TakeLog(log_tx) => {
                        let _ = log_tx.send(take(&mut self.log));
                        continue 'signup;
                    }
                    input @ Input::InitializeSignup { .. } => {
                        bail!("Unexpected image_notary input: {input:?}")
                    }
                }
            }
            break;
        }
        Ok(())
    }
}

impl Agent {
    fn initialize_signup(&mut self) {
        self.save_dir = Path::new(DATA_ACQUISITION_BASE_DIR).join(self.signup_id.to_string());
        #[cfg(feature = "internal-data-acquisition")]
        ssd::perform(|| std::fs::create_dir_all(&self.save_dir));
        self.last_ir_save_time = Duration::ZERO;
        self.last_ir_face_save_time = Duration::ZERO;
        self.last_rgb_save_time = Duration::ZERO;
        self.last_thermal_save_time = Duration::ZERO;
        self.log = Log::default();
        self.sharpest_frames = SharpnessHeaps::default();
    }

    #[cfg(not(feature = "internal-data-acquisition"))]
    #[allow(clippy::unnecessary_wraps)]
    fn finalize_signup(&mut self) -> Result<()> {
        // In theory this is not needed. Just an extra defensive measure.
        ssd::perform(|| {
            self.save_dir.exists().then(|| remove_dir_all(&self.save_dir)).unwrap_or(Ok(()))
        });
        Ok(())
    }

    #[cfg(feature = "internal-data-acquisition")]
    fn finalize_signup(&mut self) -> Result<()> {
        ssd_save_png(|| {
            save_sharpest_frames(
                &self.sharpest_frames,
                &self.signup_id,
                &self.save_dir,
                &mut self.log,
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn handle_save_identification_images(
        &self,
        input: SaveIdentificationImagesInput,
    ) -> Result<()> {
        let SaveIdentificationImagesInput { tx, left, right, self_custody_candidate } = input;
        let mut identification_images = None;
        ssd_save_png(|| {
            identification_images = Some(save_identification_images_impl(
                &self.signup_id,
                &self.save_dir,
                &left,
                &right,
                &self_custody_candidate,
            )?);
            Ok(())
        })?;
        if identification_images.is_some() {
            dd_incr!("main.count.data_acquisition.identification_images.saved");
        } else {
            tracing::error!("Storing identification images failed. Please check SSD state.");
            dd_incr!("main.count.data_acquisition.identification_images.failed");
        }
        let _ = tx.send(identification_images);
        Ok(())
    }

    #[allow(clippy::too_many_arguments, unused_variables, clippy::unnecessary_wraps)]
    fn handle_save_ir_net_estimate(
        &mut self,
        input: SaveIrNetEstimateInput,
        port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveIrNetEstimateInput {
            estimate,
            frame,
            wavelength,
            target_left_eye,
            fps_override,
            log_metadata_always,
        } = input;
        let image_id = frame.image_id(&self.signup_id);
        let ir_net_metadata = estimate.clone().map(Into::into);
        // helper closure
        let log_metadata = |image_id, saved| {
            self.log.ir_net_metadata.push((
                image_id,
                ir_net_metadata,
                wavelength,
                target_left_eye,
                frame.timestamp(),
                saved,
            ));
        };

        #[cfg(not(feature = "internal-data-acquisition"))]
        {
            if log_metadata_always {
                log_metadata(image_id, false);
            }
            Ok(())
        }

        #[cfg(feature = "internal-data-acquisition")]
        {
            if frame.is_empty()
                || !sample_at_fps(
                    fps_override.unwrap_or(IR_EYE_SAVE_FPS),
                    frame.timestamp(),
                    self.last_ir_save_time,
                )
            {
                if log_metadata_always {
                    log_metadata(image_id, false);
                }
                return Ok(());
            }
            ssd_save_png(|| {
                let frame_path =
                    save_frame_with_id(&image_id, &frame, &self.save_dir.join("ir_camera"), None)?;
                self.last_ir_save_time = frame.timestamp();
                log_metadata(image_id, true);
                #[cfg(not(test))]
                let _ = frame_path;
                #[cfg(not(test))]
                let _ = port;
                #[cfg(test)]
                runtime::Runtime::new()?.block_on(async {
                    port.send(port::Output::new(Output::IrEyeFramePath(frame_path))).await.unwrap();
                });
                Ok(())
            })?;

            // update sharpest frames
            if estimate.as_ref().is_some_and(|e| e.score < IRIS_SCORE_MIN) {
                return Ok(());
            }
            let map = self.sharpest_frames.entry((wavelength, target_left_eye)).or_default();
            if map.len() == NUM_SHARP_IR_FRAMES {
                map.pop_first();
            }
            let score = estimate.as_ref().map_or(0.0, |e| e.score);
            map.insert(OrderedFloat(score), (estimate, frame.clone()));
            Ok(())
        }
    }

    #[allow(unused_variables, clippy::unnecessary_wraps)]
    fn handle_save_ir_face_data(
        &mut self,
        input: SaveIrFaceDataInput,
        port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveIrFaceDataInput { frame, wavelength, fps_override, log_metadata_always } = input;
        let image_id = frame.image_id(&self.signup_id);
        // helper closure
        let mut log_metadata = |image_id, saved| {
            self.log.ir_face_metadata.push((image_id, wavelength, frame.timestamp(), saved));
        };

        #[cfg(not(feature = "internal-data-acquisition"))]
        {
            if log_metadata_always {
                log_metadata(image_id, false);
            }
            Ok(())
        }

        #[cfg(feature = "internal-data-acquisition")]
        {
            if frame.is_empty()
                || !sample_at_fps(
                    fps_override.unwrap_or(IR_FACE_SAVE_FPS),
                    frame.timestamp(),
                    self.last_ir_face_save_time,
                )
            {
                if log_metadata_always {
                    log_metadata(image_id, false);
                }
                return Ok(());
            }
            ssd_save_png(|| {
                let frame_path =
                    save_frame_with_id(&image_id, &frame, &self.save_dir.join("ir_face"), None)?;
                self.last_ir_face_save_time = frame.timestamp();
                log_metadata(image_id, true);
                #[cfg(not(test))]
                let _ = frame_path;
                #[cfg(not(test))]
                let _ = port;
                #[cfg(test)]
                runtime::Runtime::new()?.block_on(async {
                    port.send(port::Output::new(Output::IrFaceFramePath(frame_path)))
                        .await
                        .unwrap();
                });
                Ok(())
            })?;
            Ok(())
        }
    }

    #[allow(unused_variables, clippy::unnecessary_wraps)]
    fn handle_save_rgb_net_estimate(&mut self, input: SaveRgbNetEstimateInput) -> Result<()> {
        let SaveRgbNetEstimateInput { estimate, frame, log_metadata_always, resolution_override } =
            input;
        let image_id = frame.image_id(&self.signup_id);
        let rgb_net_metadata = estimate.clone();
        // helper closure
        let log_metadata = |image_id, saved| {
            self.log.rgb_net_metadata.push((image_id, rgb_net_metadata, frame.timestamp(), saved));
        };

        #[cfg(not(feature = "internal-data-acquisition"))]
        {
            if log_metadata_always {
                log_metadata(image_id, false);
            }
            Ok(())
        }

        #[cfg(feature = "internal-data-acquisition")]
        {
            if frame.as_bytes().is_empty()
                || !sample_at_fps(RGB_SAVE_FPS, frame.timestamp(), self.last_rgb_save_time)
            {
                if log_metadata_always {
                    log_metadata(image_id, false);
                }
                return Ok(());
            }
            ssd_save_png(|| {
                self.last_rgb_save_time = frame.timestamp();
                save_frame_with_id(
                    &image_id,
                    &frame,
                    &self.save_dir.join("rgb_camera"),
                    resolution_override.or(Some(FrameResolution::LOW)),
                )?;
                log_metadata(image_id, true);
                Ok(())
            })?;
            Ok(())
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn handle_save_fusion_rn_fi(&mut self, input: SaveFusionRnFiInput) -> Result<()> {
        let SaveFusionRnFiInput { estimate, is_valid, frame, log_metadata_always } = input;
        let image_id = frame.image_id(&self.signup_id);
        let rgb_net_metadata = estimate.clone();
        let face_identifier_metadata = is_valid.clone().into();
        // helper closure
        let log_metadata = |image_id: ImageId, saved| {
            self.log.rgb_net_metadata.push((
                image_id.clone(),
                rgb_net_metadata,
                frame.timestamp(),
                saved,
            ));
            self.log.face_identifier_metadata.push((
                image_id,
                face_identifier_metadata,
                frame.timestamp(),
                saved,
            ));
        };

        #[cfg(not(feature = "internal-data-acquisition"))]
        {
            if log_metadata_always {
                log_metadata(image_id, false);
            }
            Ok(())
        }

        #[cfg(feature = "internal-data-acquisition")]
        {
            if frame.as_bytes().is_empty()
                || !sample_at_fps(RGB_SAVE_FPS, frame.timestamp(), self.last_rgb_save_time)
            {
                if log_metadata_always {
                    log_metadata(image_id, false);
                }
                return Ok(());
            }
            ssd_save_png(|| {
                self.last_rgb_save_time = frame.timestamp();
                save_frame_with_id(
                    &image_id,
                    &frame,
                    &self.save_dir.join("rgb_camera"),
                    Some(FrameResolution::LOW),
                )?;
                log_metadata(image_id, true);
                Ok(())
            })?;
            Ok(())
        }
    }

    #[allow(unused_variables, clippy::unnecessary_wraps)]
    fn handle_save_thermal_data(
        &mut self,
        input: SaveThermalDataInput,
        port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveThermalDataInput { frame, wavelength, fps_override, log_metadata_always } = input;
        let image_id = frame.image_id(&self.signup_id);
        // helper closure
        let mut log_metadata = |image_id, saved| {
            self.log.thermal_metadata.push((image_id, wavelength, frame.timestamp(), saved));
        };

        #[cfg(not(feature = "internal-data-acquisition"))]
        {
            if log_metadata_always {
                log_metadata(image_id, false);
            }
            Ok(())
        }

        #[cfg(feature = "internal-data-acquisition")]
        {
            if frame.is_empty()
                || !sample_at_fps(
                    fps_override.unwrap_or(THERMAL_SAVE_FPS),
                    frame.timestamp(),
                    self.last_thermal_save_time,
                )
            {
                if log_metadata_always {
                    log_metadata(image_id, false);
                }
                return Ok(());
            }
            ssd_save_png(|| {
                let frame_path =
                    save_frame_with_id(&image_id, &frame, &self.save_dir.join("thermal"), None)?;
                self.last_thermal_save_time = frame.timestamp();
                log_metadata(image_id, true);
                #[cfg(not(test))]
                let _ = frame_path;
                #[cfg(not(test))]
                let _ = port;
                #[cfg(test)]
                runtime::Runtime::new()?.block_on(async {
                    port.send(port::Output::new(Output::ThermalFramePath(frame_path)))
                        .await
                        .unwrap();
                });
                Ok(())
            })?;
            Ok(())
        }
    }

    fn handle_get_sharpest_frame(&mut self, input: GetSharpestFrameInput) {
        let GetSharpestFrameInput { wavelength, side, tx } = input;
        let _ = tx.send(self.sharpest_frames.get_mut(&(wavelength, side)).and_then(|heap| {
            heap.keys()
                .next_back()
                .and_then(|last| heap.get(last).and_then(|(estimate, _)| estimate.clone()))
        }));
    }
}

#[cfg(feature = "internal-data-acquisition")]
fn save_sharpest_frames(
    sharpest_frames: &SharpnessHeaps,
    signup_id: &SignupId,
    save_dir: &Path,
    log: &mut Log,
) -> Result<(), EncodingError> {
    for ((wavelength, side), sharpness_heap) in sharpest_frames {
        for (score, (ir_net_estimate, frame)) in sharpness_heap {
            tracing::debug!(
                "Saving {} sharpest frames for wavelength {:?}, side {:?}, with score {:?}",
                sharpness_heap.len(),
                wavelength,
                side,
                score
            );
            let image_id = frame.image_id(signup_id);
            save_frame_with_id(&image_id, frame, &save_dir.join("ir_camera"), None)?;
            log.ir_net_metadata.push((
                image_id,
                ir_net_estimate.clone().map(Into::into),
                *wavelength,
                *side,
                frame.timestamp(),
                true,
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn save_identification_images_impl(
    signup_id: &SignupId,
    save_dir: &Path,
    left: &EyeCapture,
    right: &EyeCapture,
    self_custody_candidate: &SelfCustodyCandidate,
) -> Result<IdentificationImages, EncodingError> {
    let left_ir = left.ir_frame.image_id(signup_id);
    save_frame_with_id(
        &left_ir,
        &left.ir_frame,
        &save_dir.join("identification").join("ir").join("left"),
        None,
    )?;
    let left_ir_940nm = left
        .ir_frame_940nm
        .as_ref()
        .map(|frame| {
            let image_id = frame.image_id(signup_id);
            save_frame_with_id(
                &image_id,
                frame,
                &save_dir.join("identification").join("ir").join("left_940nm"),
                None,
            )
            .map(|_| image_id)
        })
        .transpose()?;
    let left_ir_740nm = left
        .ir_frame_740nm
        .as_ref()
        .map(|frame| {
            let image_id = frame.image_id(signup_id);
            save_frame_with_id(
                &image_id,
                frame,
                &save_dir.join("identification").join("ir").join("left_740nm"),
                None,
            )
            .map(|_| image_id)
        })
        .transpose()?;
    let right_ir = right.ir_frame.image_id(signup_id);
    save_frame_with_id(
        &right_ir,
        &right.ir_frame,
        &save_dir.join("identification").join("ir").join("right"),
        None,
    )?;
    let right_ir_940nm = right
        .ir_frame_940nm
        .as_ref()
        .map(|frame| {
            let image_id = frame.image_id(signup_id);
            save_frame_with_id(
                &image_id,
                frame,
                &save_dir.join("identification").join("ir").join("right_940nm"),
                None,
            )
            .map(|_| image_id)
        })
        .transpose()?;
    let right_ir_740nm = right
        .ir_frame_740nm
        .as_ref()
        .map(|frame| {
            let image_id = frame.image_id(signup_id);
            save_frame_with_id(
                &image_id,
                frame,
                &save_dir.join("identification").join("ir").join("right_740nm"),
                None,
            )
            .map(|_| image_id)
        })
        .transpose()?;
    let left_rgb = left.rgb_frame.image_id(signup_id);
    save_frame_with_id(
        &left_rgb,
        &left.rgb_frame,
        &save_dir.join("identification").join("rgb").join("left"),
        Some(FrameResolution::MEDIUM),
    )?;
    let left_rgb_fullres = left.rgb_frame.image_id(signup_id);
    save_frame_with_id(
        &left_rgb_fullres,
        &left.rgb_frame,
        &save_dir.join("rgb_camera"),
        Some(FrameResolution::MAX),
    )?;
    let right_rgb = right.rgb_frame.image_id(signup_id);
    save_frame_with_id(
        &right_rgb,
        &right.rgb_frame,
        &save_dir.join("identification").join("rgb").join("right"),
        Some(FrameResolution::LOW),
    )?;
    let right_rgb_fullres = right.rgb_frame.image_id(signup_id);
    save_frame_with_id(
        &right_rgb_fullres,
        &right.rgb_frame,
        &save_dir.join("rgb_camera"),
        Some(FrameResolution::MAX),
    )?;
    let self_custody_candidate_id = self_custody_candidate.rgb_frame.image_id(signup_id);
    save_frame_with_id(
        &self_custody_candidate_id,
        &self_custody_candidate.rgb_frame,
        &save_dir.join("identification").join("rgb").join("self_custody_candidate"),
        Some(FrameResolution::MAX),
    )?;
    Ok(IdentificationImages {
        left_ir,
        left_ir_940nm,
        left_ir_740nm,
        right_ir,
        right_ir_940nm,
        right_ir_740nm,
        left_rgb,
        left_rgb_fullres,
        right_rgb,
        right_rgb_fullres,
        self_custody_candidate: self_custody_candidate_id,
    })
}

#[allow(clippy::unnecessary_wraps)]
fn save_frame_with_id(
    image_id: &ImageId,
    #[allow(unused_variables)] frame: &impl Frame,
    save_dir: &Path,
    #[allow(unused_variables)] resolution: Option<FrameResolution>,
) -> Result<PathBuf, EncodingError> {
    let frame_path = save_dir.join(image_id.to_string()).with_extension("png");
    #[cfg(any(feature = "internal-data-acquisition", test))]
    {
        tracing::trace!("Writing frame to {frame_path:?}");
        save_frame(frame, &frame_path, resolution)?;
    }
    #[cfg(not(any(feature = "internal-data-acquisition", test)))]
    {
        tracing::trace!("Pretending to write frame to {frame_path:?}");
    }
    Ok(frame_path)
}

#[cfg(any(feature = "internal-data-acquisition", test))]
fn save_frame(
    frame: &impl Frame,
    file_path: &Path,
    resolution: Option<FrameResolution>,
) -> Result<(), EncodingError> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(file_path)?;
    #[cfg(feature = "no-image-encryption")]
    {
        frame.write_png(&mut file, resolution.unwrap_or_default())?;
    }
    #[cfg(not(feature = "no-image-encryption"))]
    {
        use std::io::Write;
        let mut png_buf = std::io::Cursor::new(Vec::new());
        frame.write_png(&mut png_buf, resolution.unwrap_or_default())?;
        let encrypted_frame = encrypt_and_seal(&png_buf.into_inner());
        file.write_all(&encrypted_frame)?;
    }
    Ok(())
}

fn ssd_save_png<R, F: FnOnce() -> Result<R, EncodingError>>(
    f: F,
) -> Result<Option<R>, EncodingError> {
    if !is_enough_space() {
        tracing::warn!("Image notary failed to write: Not enough space to SSD");
        dd_incr!("main.count.data_acquisition.ssd_full");
        return Ok(None);
    }

    ssd::perform(|| match f() {
        Ok(value) => Ok(Ok(Some(value))),
        Err(EncodingError::IoError(err)) => Err(err),
        Err(err) => Ok(Err(err)),
    })
    .map_or(Ok(None), identity)
}

fn is_enough_space() -> bool {
    ssd::available_space() > MIN_AVAILABLE_SSD_SPACE
}

fn ensure_enough_space() -> Result<()> {
    while ssd::available_space() < MIN_AVAILABLE_SSD_SPACE_BEFORE_SIGNUP {
        let mut oldest_entry_path = None;
        let mut oldest_entry_created = None;
        ssd::perform(|| {
            if let Err(e) = std::fs::create_dir_all(Path::new(DATA_ACQUISITION_BASE_DIR)) {
                tracing::error!(
                    "Failed to create {DATA_ACQUISITION_BASE_DIR} (ssd stats: {:?}, is mounted: \
                     {}): {e}",
                    ssd::stats(),
                    ssd::is_mounted()
                );
                Err(e)
            } else {
                Ok(())
            }
        });
        for entry in WalkDir::new(DATA_ACQUISITION_BASE_DIR).min_depth(1).max_depth(1) {
            let Ok(entry) = entry else {
                tracing::error!(
                    "walking: {DATA_ACQUISITION_BASE_DIR} failed (ssd stats: {:?}, is mounted: {})",
                    ssd::stats(),
                    ssd::is_mounted()
                );
                return Ok(());
            };
            let entry_created = entry.metadata()?.created()?;
            if oldest_entry_created.map_or(true, |oldest| entry_created < oldest) {
                oldest_entry_created = Some(entry_created);
                oldest_entry_path = Some(entry.path().to_owned());
            }
        }
        if let (Some(oldest_entry_path), Some(oldest_entry_created)) =
            (oldest_entry_path, oldest_entry_created)
        {
            tracing::info!(
                "Removing signup dir {} created at {}",
                oldest_entry_path.display(),
                OffsetDateTime::from(oldest_entry_created).format(&Rfc2822)?,
            );
            dd_incr!("main.count.data_acquisition.cleanups");
            fs::remove_dir_all(oldest_entry_path)?;
        } else {
            tracing::error!(
                "Not enough space on the SSD while {DATA_ACQUISITION_BASE_DIR} is empty"
            );
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "stage")]
mod tests {
    use super::*;
    use crate::{
        agents::{camera, image_notary},
        ext::mpsc::SenderExt as _,
        logger,
        timestamped::Timestamped,
    };
    use agentwire::agent::Thread as _;
    use eyre::{ensure, WrapErr};
    use orb_wld_data_id::{ImageId, S3Region, SignupId};
    use std::io::Cursor;
    use tokio::time::timeout;

    /// # Panics
    /// If the ciphertext and plaintext are the same, or if decryption failed.
    #[cfg(not(feature = "no-image-encryption"))]
    fn decrypt_and_unseal(ciphertext: &[u8]) -> Vec<u8> {
        use sodiumoxide::crypto::sealedbox;

        let plaintext = sealedbox::open(
            ciphertext,
            &crate::consts::WORLDCOIN_ENCRYPTION_PUBKEY,
            &crate::consts::WORLDCOIN_ENCRYPTION_SECRETKEY,
        )
        .expect("Failed to decrypt sealedbox");
        assert_ne!(ciphertext, plaintext);
        plaintext
    }

    fn init_image_notary(signup_id: SignupId) -> Result<port::Outer<Agent>> {
        let (mut image_notary, _) = Agent::default().spawn_thread()?;
        image_notary.tx.send_now(port::Input::new(Input::InitializeSignup { signup_id }))?;
        Ok(image_notary)
    }

    macro_rules! tout {
        ($fut:expr) => {
            timeout(Duration::from_millis(5000), $fut).map(|output| output.expect("timed out"))
        };
    }

    #[tokio::test]
    async fn test_save_ir() -> Result<()> {
        logger::init::<false>();
        let signup_id = SignupId::new(S3Region::Unknown);
        let mut image_notary = init_image_notary(signup_id)?;

        // Expected inputs
        let expected_frame = camera::ir::Frame::default();
        let mut expected_bytes = Vec::new();
        expected_frame
            .write_png(Cursor::new(&mut expected_bytes), FrameResolution::MAX)
            .wrap_err("Failed to encode expected frame to png")?;
        let expected_wavelength = IrLed::L850;

        image_notary
            .send(port::Input::new(Input::SaveIrNetEstimate(SaveIrNetEstimateInput {
                estimate: None,
                frame: expected_frame.clone(),
                wavelength: expected_wavelength,
                target_left_eye: false,
                fps_override: Some(f32::INFINITY),
                log_metadata_always: true,
            })))
            .await?;
        let output = tout!(image_notary.next()).await.expect("no item");
        let Output::IrEyeFramePath(frame_path) = output.value else {
            panic!("Expected an IR eye path");
        };

        // Check that bytes match
        let check_output = async {
            let saved_bytes = tokio::fs::read(&frame_path)
                .await
                .wrap_err("Failed to read file the image notary should have produced")?;
            #[cfg(not(feature = "no-image-encryption"))]
            let saved_bytes = decrypt_and_unseal(&saved_bytes);
            ensure!(expected_bytes == saved_bytes, "saved bytes didn't match");

            // Check that saved frame properties make sense.
            let saved_frame = camera::ir::Frame::read_png(saved_bytes.as_slice())
                .wrap_err("Failed to decode saved frame")?;
            ensure!(saved_frame.width() == expected_frame.width(), "width did not match");
            ensure!(saved_frame.height() == expected_frame.height(), "height did not match");
            ensure!(*saved_frame == *expected_frame, "decoded frame data did not match");

            // Check that we logged the frame
            let image_id = ImageId::from_image_path(&frame_path)?;
            let mut image_notary_log = image_notary::take_log(&mut image_notary).await?;
            let image_id_log = image_notary_log
                .ir_net_metadata
                .iter()
                .find(|Timestamped { value: (id, _, _, _, _, _), .. }| *id == image_id.clone());
            let Some(image_id_log) = image_id_log else {
                bail!("image_id was not logged");
            };
            ensure!(image_id_log.value.1.is_none(), "IrNetMetada was not logged correctly");
            ensure!(
                image_id_log.value.2 == expected_wavelength,
                "wavelength was not logged correctly"
            );

            Ok(())
        };
        let result = check_output.await;
        //cleanup image
        let _ = tokio::fs::remove_dir_all(frame_path).await;
        result
    }

    #[tokio::test]
    async fn test_save_ir_face() -> Result<()> {
        logger::init::<false>();
        let signup_id = SignupId::new(S3Region::Unknown);
        let mut image_notary = init_image_notary(signup_id)?;

        // Expected inputs
        let expected_frame = camera::ir::Frame::default();
        let mut expected_bytes = Vec::new();
        expected_frame
            .write_png(Cursor::new(&mut expected_bytes), FrameResolution::MAX)
            .wrap_err("Failed to encode expected frame to png")?;
        let expected_wavelength = IrLed::L940;

        // Do test
        image_notary
            .send(port::Input::new(Input::SaveIrFaceData(SaveIrFaceDataInput {
                frame: expected_frame.clone(),
                wavelength: expected_wavelength,
                fps_override: Some(f32::INFINITY),
                log_metadata_always: true,
            })))
            .await
            .wrap_err("Failed to send example to image notary")?;
        let output = tout!(image_notary.next()).await.expect("no item");
        let Output::IrFaceFramePath(frame_path) = output.value else {
            panic!("Expected an IR face path");
        };

        let check_output = async {
            // Check that bytes match
            let saved_bytes = tokio::fs::read(&frame_path)
                .await
                .wrap_err("Failed to read file the image notary should have produced")?;
            #[cfg(not(feature = "no-image-encryption"))]
            let saved_bytes = decrypt_and_unseal(&saved_bytes);
            ensure!(expected_bytes == saved_bytes, "saved bytes didn't match");

            // Check that saved frame properties make sense.
            let saved_frame = camera::ir::Frame::read_png(saved_bytes.as_slice())
                .wrap_err("Failed to decode saved frame")?;
            ensure!(saved_frame.width() == expected_frame.width(), "width did not match");
            ensure!(saved_frame.height() == expected_frame.height(), "height did not match");
            ensure!(*saved_frame == *expected_frame, "decoded frame data did not match");

            // Check that we logged the frame
            let image_id = ImageId::from_image_path(&frame_path)?;
            let mut image_notary_log = image_notary::take_log(&mut image_notary).await?;
            let image_id_log = image_notary_log
                .ir_face_metadata
                .iter()
                .find(|Timestamped { value: (id, _, _, _), .. }| *id == image_id.clone());
            let Some(image_id_log) = image_id_log else {
                bail!("image_id was not logged");
            };
            ensure!(image_id_log.1 == expected_wavelength, "wavelength was not logged correctly");

            Ok(())
        };
        let result = check_output.await;
        //cleanup image
        let _ = tokio::fs::remove_dir_all(frame_path).await;
        result
    }

    #[tokio::test]
    async fn test_save_thermal() -> Result<()> {
        logger::init::<false>();
        let signup_id = SignupId::new(S3Region::Unknown);
        let mut image_notary = init_image_notary(signup_id)?;

        // Expected inputs
        let expected_frame = camera::thermal::Frame::default();
        let mut expected_bytes = Vec::new();
        expected_frame
            .write_png(Cursor::new(&mut expected_bytes), FrameResolution::MAX)
            .wrap_err("Failed to encode expected frame to png")?;
        let expected_wavelength = IrLed::L940;

        // Do test
        image_notary
            .send(port::Input::new(Input::SaveThermalData(SaveThermalDataInput {
                frame: expected_frame.clone(),
                wavelength: expected_wavelength,
                fps_override: Some(f32::INFINITY),
                log_metadata_always: true,
            })))
            .await
            .wrap_err("Failed to send example to image notary")?;
        let output = tout!(image_notary.next()).await.expect("no item");
        let Output::ThermalFramePath(frame_path) = output.value else {
            panic!("Expected a thermal frame path");
        };

        let check_output = async {
            // Check that bytes match
            let saved_bytes = tokio::fs::read(&frame_path)
                .await
                .wrap_err("Failed to read file the image notary should have produced")?;
            #[cfg(not(feature = "no-image-encryption"))]
            let saved_bytes = decrypt_and_unseal(&saved_bytes);
            ensure!(expected_bytes == saved_bytes, "saved bytes didn't match");

            // Check that saved frame properties make sense.
            let saved_frame = camera::thermal::Frame::read_png(saved_bytes.as_slice())
                .wrap_err("Failed to decode saved frame")?;
            ensure!(saved_frame.width() == expected_frame.width(), "width did not match");
            ensure!(saved_frame.height() == expected_frame.height(), "height did not match");
            ensure!(**saved_frame == **expected_frame, "decoded frame data did not match");

            // Check that we logged the frame
            let image_id = ImageId::from_image_path(&frame_path)?;
            let mut image_notary_log = image_notary::take_log(&mut image_notary).await?;
            let image_id_log = image_notary_log
                .thermal_metadata
                .iter()
                .find(|Timestamped { value: (id, _, _, _), .. }| *id == image_id.clone());
            let Some(image_id_log) = image_id_log else {
                bail!("image_id was not logged");
            };
            ensure!(image_id_log.1 == expected_wavelength, "wavelength was not logged correctly");

            Ok(())
        };
        let result = check_output.await;
        //cleanup image
        let _ = tokio::fs::remove_dir_all(frame_path).await;
        result
    }
}
