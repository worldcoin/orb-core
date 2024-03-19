//! Notarizes images.

use crate::{
    agents::{
        camera::{self, Frame, FrameResolution},
        python::{self, rgb_net},
    },
    consts::{
        IRIS_SCORE_MIN, IR_EYE_SAVE_FPS, IR_FACE_SAVE_FPS, NUM_SHARP_IR_FRAMES, RGB_SAVE_FPS,
        THERMAL_SAVE_FPS,
    },
    debug_report::{FaceIdentifierIsValidMetadata, IrNetMetadata},
    logger::{DATADOG, NO_TAGS},
    mcu::main::IrLed,
    plans::biometric_capture::{EyeCapture, SelfCustodyCandidate},
    port,
    port::Port,
    time_series::TimeSeries,
    utils::sample_at_fps,
};
use eyre::{bail, Result};
use futures::{channel::oneshot, prelude::*};
use orb_wld_data_id::{ImageId, SignupId};
use ordered_float::OrderedFloat;
#[cfg(not(test))]
use std::convert::Infallible;
use std::{
    clone::Clone,
    collections::{BTreeMap, HashMap},
    mem::take,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::runtime;

type SharpnessHeaps = HashMap<
    (IrLed, bool),
    BTreeMap<OrderedFloat<f64>, (Option<python::ir_net::EstimateOutput>, camera::ir::Frame)>,
>;

#[allow(missing_docs)]
#[derive(Default, Debug)]
pub struct Agent {
    signup_id: SignupId,
    is_opt_in: bool,
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
        is_opt_in: bool,
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
    pub ir_net_metadata:
        TimeSeries<(Option<ImageId>, Option<IrNetMetadata>, IrLed, bool, Duration)>,
    /// IR Face metadata
    pub ir_face_metadata: TimeSeries<(Option<ImageId>, IrLed, Duration)>,
    /// RGB Net metadata history
    pub rgb_net_metadata: TimeSeries<(Option<ImageId>, rgb_net::EstimateOutput, Duration)>,
    /// Face Identifier metadata history
    pub face_identifier_metadata:
        TimeSeries<(Option<ImageId>, FaceIdentifierIsValidMetadata, Duration)>,
    /// Thermal metadata
    pub thermal_metadata: TimeSeries<(Option<ImageId>, IrLed, Duration)>,
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

impl port::Outer<Agent> {
    /// Gets the sharpest frame seen since the last `InitializeSignup` message
    /// was received.
    pub async fn get_sharpest_frame(
        &mut self,
        wavelength: IrLed,
        target_left_eye: bool,
    ) -> Result<Option<python::ir_net::EstimateOutput>> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Input::GetSharpestFrame(GetSharpestFrameInput {
            wavelength,
            side: target_left_eye,
            tx,
        })))
        .await?;
        Ok(rx.await?)
    }

    /// Saves eye / face identification images and returns their IDs.
    pub async fn save_identification_images(
        &mut self,
        left: EyeCapture,
        right: EyeCapture,
        self_custody_candidate: SelfCustodyCandidate,
    ) -> Result<Option<IdentificationImages>> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Input::SaveIdentificationImages(Box::new(
            SaveIdentificationImagesInput { tx, left, right, self_custody_candidate },
        ))))
        .await?;
        Ok(rx.await?)
    }

    /// Takes the configuration history since the last `InitializeSignup`
    /// message was received.
    pub async fn take_log(&mut self) -> Result<Log> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Input::TakeLog(tx))).await?;
        Ok(rx.await?)
    }
}

impl super::Agent for Agent {
    const NAME: &'static str = "image-saver";
}

impl super::AgentThread for Agent {
    fn run(mut self, mut port: port::Inner<Self>) -> Result<()> {
        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        'signup: while let Some(input) = rt.block_on(port.next()) {
            (self.signup_id, self.is_opt_in) = match input.value {
                Input::InitializeSignup { signup_id, is_opt_in } => (signup_id, is_opt_in),
                input => bail!("Unexpected image_notary input: {input:?}"),
            };
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
        self.last_ir_save_time = Duration::ZERO;
        self.last_ir_face_save_time = Duration::ZERO;
        self.last_rgb_save_time = Duration::ZERO;
        self.last_thermal_save_time = Duration::ZERO;
        self.log = Log::default();
        self.sharpest_frames = SharpnessHeaps::default();
    }

    #[allow(clippy::unused_self, clippy::unnecessary_wraps)] // FOSS
    fn finalize_signup(&mut self) -> Result<()> {
        Ok(())
    }

    fn handle_save_identification_images(
        &self,
        input: SaveIdentificationImagesInput,
    ) -> Result<()> {
        let SaveIdentificationImagesInput { tx, left, right, self_custody_candidate } = input;
        let identification_images = identification_images_ids(
            &self.signup_id,
            &self.save_dir,
            &left,
            &right,
            &self_custody_candidate,
        );
        DATADOG.incr("orb.main.count.data_collection.identification_images.saved", NO_TAGS)?;
        let _ = tx.send(Some(identification_images));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::unnecessary_wraps)] // FOSS
    fn handle_save_ir_net_estimate(
        &mut self,
        input: SaveIrNetEstimateInput,
        _port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveIrNetEstimateInput {
            estimate,
            frame,
            wavelength,
            target_left_eye,
            fps_override,
            log_metadata_always,
        } = input;
        let ir_net_metadata = estimate.clone().map(Into::into);
        // helper closure
        let log_metadata = |image_id| {
            self.log.ir_net_metadata.push((
                image_id,
                ir_net_metadata,
                wavelength,
                target_left_eye,
                frame.timestamp(),
            ));
        };
        if !self.is_opt_in
            || frame.is_empty()
            || !sample_at_fps(
                fps_override.unwrap_or(IR_EYE_SAVE_FPS),
                frame.timestamp(),
                self.last_ir_save_time,
            )
        {
            if log_metadata_always {
                log_metadata(None);
            }
            return Ok(());
        }
        let image_id = get_image_id(&frame, &self.signup_id);
        self.last_ir_save_time = frame.timestamp();
        log_metadata(Some(image_id));

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

    #[allow(clippy::unnecessary_wraps)] // FOSS
    fn handle_save_ir_face_data(
        &mut self,
        input: SaveIrFaceDataInput,
        _port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveIrFaceDataInput { frame, wavelength, fps_override, log_metadata_always } = input;
        // helper closure
        let mut log_metadata = |image_id| {
            self.log.ir_face_metadata.push((image_id, wavelength, frame.timestamp()));
        };
        if !self.is_opt_in
            || frame.is_empty()
            || !sample_at_fps(
                fps_override.unwrap_or(IR_FACE_SAVE_FPS),
                frame.timestamp(),
                self.last_ir_face_save_time,
            )
        {
            if log_metadata_always {
                log_metadata(None);
            }
            return Ok(());
        }
        let image_id = get_image_id(&frame, &self.signup_id);
        self.last_ir_face_save_time = frame.timestamp();
        log_metadata(Some(image_id));
        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)] // FOSS
    fn handle_save_rgb_net_estimate(&mut self, input: SaveRgbNetEstimateInput) -> Result<()> {
        let SaveRgbNetEstimateInput {
            estimate,
            frame,
            log_metadata_always,
            resolution_override: _,
        } = input;
        let rgb_net_metadata = estimate.clone();
        // helper closure
        let log_metadata = |image_id| {
            self.log.rgb_net_metadata.push((image_id, rgb_net_metadata, frame.timestamp()));
        };
        if !self.is_opt_in
            || frame.is_empty()
            || !sample_at_fps(RGB_SAVE_FPS, frame.timestamp(), self.last_rgb_save_time)
        {
            if log_metadata_always {
                log_metadata(None);
            }
            return Ok(());
        }
        self.last_rgb_save_time = frame.timestamp();
        let image_id = get_image_id(&frame, &self.signup_id);
        log_metadata(Some(image_id));
        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)] // FOSS
    fn handle_save_fusion_rn_fi(&mut self, input: SaveFusionRnFiInput) -> Result<()> {
        let SaveFusionRnFiInput { estimate, is_valid, frame, log_metadata_always } = input;
        let rgb_net_metadata = estimate.clone();
        let face_identifier_metadata = is_valid.clone().into();
        // helper closure
        let log_metadata = |image_id: Option<ImageId>| {
            self.log.rgb_net_metadata.push((image_id.clone(), rgb_net_metadata, frame.timestamp()));
            self.log.face_identifier_metadata.push((
                image_id,
                face_identifier_metadata,
                frame.timestamp(),
            ));
        };

        if !self.is_opt_in
            || frame.is_empty()
            || !sample_at_fps(RGB_SAVE_FPS, frame.timestamp(), self.last_rgb_save_time)
        {
            if log_metadata_always {
                log_metadata(None);
            }
            return Ok(());
        }
        self.last_rgb_save_time = frame.timestamp();
        let image_id = get_image_id(&frame, &self.signup_id);
        log_metadata(Some(image_id));
        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)] // FOSS
    fn handle_save_thermal_data(
        &mut self,
        input: SaveThermalDataInput,
        _port: &mut port::Inner<Self>,
    ) -> Result<()> {
        let SaveThermalDataInput { frame, wavelength, fps_override, log_metadata_always } = input;
        // helper closure
        let mut log_metadata = |image_id| {
            self.log.thermal_metadata.push((image_id, wavelength, frame.timestamp()));
        };
        if !self.is_opt_in
            || frame.is_empty()
            || !sample_at_fps(
                fps_override.unwrap_or(THERMAL_SAVE_FPS),
                frame.timestamp(),
                self.last_thermal_save_time,
            )
        {
            if log_metadata_always {
                log_metadata(None);
            }
            return Ok(());
        }
        let image_id = get_image_id(&frame, &self.signup_id);
        self.last_thermal_save_time = frame.timestamp();
        log_metadata(Some(image_id));
        Ok(())
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

#[allow(clippy::too_many_lines)]
fn identification_images_ids(
    signup_id: &SignupId,
    _save_dir: &Path,
    left: &EyeCapture,
    right: &EyeCapture,
    self_custody_candidate: &SelfCustodyCandidate,
) -> IdentificationImages {
    let left_ir = get_image_id(&left.ir_frame, signup_id);
    let left_ir_940nm = left.ir_frame_940nm.as_ref().map(|frame| get_image_id(frame, signup_id));
    let left_ir_740nm = left.ir_frame_740nm.as_ref().map(|frame| get_image_id(frame, signup_id));
    let right_ir = get_image_id(&right.ir_frame, signup_id);
    let right_ir_940nm = right.ir_frame_940nm.as_ref().map(|frame| get_image_id(frame, signup_id));
    let right_ir_740nm = right.ir_frame_740nm.as_ref().map(|frame| get_image_id(frame, signup_id));
    let left_rgb = get_image_id(&left.rgb_frame, signup_id);
    let left_rgb_fullres = get_image_id(&left.rgb_frame, signup_id);
    let right_rgb = get_image_id(&right.rgb_frame, signup_id);
    let right_rgb_fullres = get_image_id(&right.rgb_frame, signup_id);
    let self_custody_candidate = get_image_id(&self_custody_candidate.rgb_frame, signup_id);
    IdentificationImages {
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
        self_custody_candidate,
    }
}

fn get_image_id(frame: &impl Frame, signup_id: &SignupId) -> ImageId {
    ImageId::new(signup_id, frame.timestamp())
}
