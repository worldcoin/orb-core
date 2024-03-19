//! Biometric pipeline.

mod code;

pub use self::code::Code;
use crate::{
    agents::{
        camera,
        python::{
            face_identifier, ir_net,
            iris::{self, Metadata, NormalizedIris},
            mega_agent_one, mega_agent_two, rgb_net,
        },
    },
    brokers::{BrokerFlow, Orb, OrbPlan},
    logger::{DATADOG, NO_TAGS},
    plans::biometric_capture::Capture,
    port,
};
use eyre::{Context as EyreContext, Result};
use futures::prelude::*;
use python_agent_interface::PyError;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::time;

// This timeout should include both boot up and function calls.
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);

const MIN_PROGRESS: f64 = 0.075;
const MAX_PROGRESS: f64 = 0.9;
const FACE_IDENTIFIER_PROGRESS: f64 = 0.031_430_735_316_813_85;
const IRIS_ESTIMATE_PROGRESS: f64 = 0.484_430_735_316_813_85;

#[allow(clippy::assertions_on_constants)]
const _: () = {
    let mut total = FACE_IDENTIFIER_PROGRESS + IRIS_ESTIMATE_PROGRESS * 2.0;
    total -= 1.0;
    if total < 0.0 {
        total = -total;
    }
    assert!(total < 0.001);
};

/// Biometric pipeline output.
#[derive(Clone, Debug)]
pub struct Pipeline {
    /// Pipeline v2 output.
    pub v2: PipelineV2,
    /// Face identifier model output for the self-custody bundle.
    pub face_identifier_bundle: Result<face_identifier::Bundle, PyError>,
    /// Mega Agent One's configuration.
    pub mega_agent_one_config: mega_agent_one::MegaAgentOne,
    /// Mega Agent One's configuration.
    pub mega_agent_two_config: mega_agent_two::MegaAgentTwo,
}

impl Pipeline {
    /// Helper function for unit tests.
    #[must_use]
    pub fn default_with_ok() -> Self {
        let config = crate::config::Config::default();
        Self {
            v2: PipelineV2::default(),
            face_identifier_bundle: Ok(face_identifier::Bundle::default()),
            mega_agent_one_config: mega_agent_one::MegaAgentOne::from(&config),
            mega_agent_two_config: mega_agent_two::MegaAgentTwo::from(&config),
        }
    }
}

/// Biometric pipeline v2 output.
#[derive(Clone, Debug, Default)]
pub struct PipelineV2 {
    /// Data for the left eye.
    pub eye_left: EyePipeline,
    /// Data for the right eye.
    pub eye_right: EyePipeline,
    /// IR-Net version.
    pub ir_net_version: String,
    /// Iris version.
    pub iris_version: String,
}

/// Processed data for one of the user's eyes.
#[derive(Clone, Debug, Default)]
pub struct EyePipeline {
    /// Iris Code.
    pub iris_code: String,
    /// Mask Code.
    pub mask_code: String,
    /// The Iris code version.
    pub iris_code_version: String,
    /// Iris metadata.
    pub metadata: Metadata,
    /// Iris normalized image.
    pub iris_normalized_image: Option<NormalizedIris>,
}

/// Biometric pipeline plan.
pub struct Plan {
    timeout: Pin<Box<time::Sleep>>,
    model_output: Option<ModelOutput>,
    eye_left: camera::ir::Frame,
    eye_right: camera::ir::Frame,
    face_left: camera::rgb::Frame,
    face_right: camera::rgb::Frame,
    face_self_custody_candidate: camera::rgb::Frame,
    face_bbox_left: rgb_net::Rectangle,
    face_bbox_right: rgb_net::Rectangle,
    face_bbox_self_custody_candidate: rgb_net::Rectangle,
    eye_landmarks_left: (rgb_net::Point, rgb_net::Point),
    eye_landmarks_right: (rgb_net::Point, rgb_net::Point),
    eye_landmarks_self_custody_candidate: (rgb_net::Point, rgb_net::Point),
}

/// Represent a biometric pipeline error.
#[derive(Error, Debug, Clone)]
#[error("Biometric pipeline Errors")]
pub enum Error {
    /// Represent a biometric pipeline timeout error.
    #[error("Timeout")]
    Timeout,
    /// Biometric pipeline failed because one or more agents received a bad or
    /// incomplete input.
    #[error("Biometric pipeline failed due to bad input")]
    Agent,
    /// Biometric pipeline failed because iris agent failed.
    #[error("Biometric pipeline failed due to iris error")]
    Iris(PyError),
}

#[allow(clippy::large_enum_variant)]
enum ModelOutput {
    MegaAgentOne(mega_agent_one::Output),
    MegaAgentTwo(mega_agent_two::Output),
    FaceIdentifier(face_identifier::Output),
    Timeout,
}

impl OrbPlan for Plan {
    fn handle_mega_agent_one(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<mega_agent_one::MegaAgentOne>,
    ) -> Result<BrokerFlow> {
        self.model_output = Some(ModelOutput::MegaAgentOne(output.value));
        Ok(BrokerFlow::Break)
    }

    fn handle_mega_agent_two(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<mega_agent_two::MegaAgentTwo>,
    ) -> Result<BrokerFlow> {
        self.model_output = Some(ModelOutput::MegaAgentTwo(output.value));
        Ok(BrokerFlow::Break)
    }

    fn handle_ir_net(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<ir_net::Model>,
        frame: Option<camera::ir::Frame>,
    ) -> Result<BrokerFlow> {
        if frame.is_some() {
            tracing::error!("There should be no frame input for IR-NET during biometric pipeline!");
        }
        self.model_output =
            Some(ModelOutput::MegaAgentOne(mega_agent_one::Output::IRNet(output.value)));
        Ok(BrokerFlow::Break)
    }

    fn handle_face_identifier(
        &mut self,
        _orb: &mut Orb,
        output: port::Output<face_identifier::Model>,
        frame: Option<camera::rgb::Frame>,
    ) -> Result<BrokerFlow> {
        if frame.is_some() {
            tracing::error!(
                "There should be no frame input for FaceIdentifier during biometric pipeline!"
            );
        }
        self.model_output = Some(ModelOutput::FaceIdentifier(output.value));
        Ok(BrokerFlow::Break)
    }

    fn poll_extra(&mut self, _orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        if let Poll::Ready(()) = self.timeout.poll_unpin(cx) {
            self.model_output = Some(ModelOutput::Timeout);
            return Ok(BrokerFlow::Break);
        }
        Ok(BrokerFlow::Continue)
    }
}

impl Plan {
    /// Creates a new biometric process plan.
    ///
    /// # Panics
    ///
    /// If RGB-Net estimate doesn't contain predictions.
    pub fn new(capture: &Capture) -> Result<Self> {
        Ok(Self {
            timeout: Box::pin(time::sleep(MODEL_TIMEOUT)),
            model_output: None,
            eye_left: capture.eye_left.ir_frame.clone(),
            eye_right: capture.eye_right.ir_frame.clone(),
            face_left: capture.eye_left.rgb_frame.clone(),
            face_right: capture.eye_right.rgb_frame.clone(),
            face_self_custody_candidate: capture.face_self_custody_candidate.rgb_frame.clone(),
            face_bbox_left: capture
                .eye_left
                .rgb_net_estimate
                .primary()
                .expect("prediction should be guaranteed by capture phase")
                .bbox
                .coordinates,
            face_bbox_right: capture
                .eye_right
                .rgb_net_estimate
                .primary()
                .expect("prediction should be guaranteed by capture phase")
                .bbox
                .coordinates,
            face_bbox_self_custody_candidate: capture.face_self_custody_candidate.rgb_net_bbox,
            eye_landmarks_left: capture
                .eye_left
                .rgb_net_estimate
                .primary()
                .map(|prediction| (prediction.landmarks.left_eye, prediction.landmarks.right_eye))
                .expect("prediction should be guaranteed by capture phase"),
            eye_landmarks_right: capture
                .eye_right
                .rgb_net_estimate
                .primary()
                .map(|prediction| (prediction.landmarks.left_eye, prediction.landmarks.right_eye))
                .expect("prediction should be guaranteed by capture phase"),
            eye_landmarks_self_custody_candidate: capture
                .face_self_custody_candidate
                .rgb_net_eye_landmarks,
        })
    }

    /// Runs the biometric process plan.
    #[allow(clippy::too_many_lines)]
    pub async fn run(&mut self, orb: &mut Orb) -> Result<Pipeline> {
        orb.enable_mega_agent_one().await?;

        let mut iris_left = None;
        let mut iris_right = None;
        let mut iris_version = None;
        let mut ir_net_version = None;
        let mut face_identifier_bundle = None;
        let mut face_identifier_update_config = None;
        let mut mega_agent_one_config = None;
        let mut mega_agent_two_config = None;
        let fence = Instant::now(); // every output for inputs earlier than this will be discarded
        let mut progress = 0.0;

        let now = Instant::now();

        // Get the Iris version.
        self.run_mega_agent_one(orb, mega_agent_one::Input::Iris(iris::Input::Version)).await?;
        self.run_mega_agent_one(orb, mega_agent_one::Input::IRNet(ir_net::Input::Version)).await?;

        self.run_update_all_configs(orb).await?;

        // Request Mega Agent's full configuration.
        self.run_mega_agent_one(orb, mega_agent_one::Input::Config).await?;
        self.run_mega_agent_two(orb, mega_agent_two::Input::Config).await?;

        // Start biometric data processing
        self.run_iris_left(orb).await?;
        self.run_iris_right(orb).await?;
        self.run_face_identifier(orb).await?;
        self.set_timeout();

        while iris_version.is_none()
            || ir_net_version.is_none()
            || face_identifier_bundle.is_none()
            || mega_agent_one_config.is_none()
            || mega_agent_two_config.is_none()
            || iris_left.is_none()
            || iris_right.is_none()
        {
            orb.run_with_fence(self, fence).await?;
            match self.model_output.take().unwrap() {
                ModelOutput::FaceIdentifier(output) => match output {
                    face_identifier::Output::Estimate { bundle } => {
                        face_identifier_bundle = Some(Ok(bundle));
                        progress += FACE_IDENTIFIER_PROGRESS;
                    }
                    face_identifier::Output::Error(error) => {
                        face_identifier_bundle = Some(Err(error.clone()));
                    }
                    o @ (face_identifier::Output::Warmup
                    | face_identifier::Output::IsValidImage(_)) => {
                        unreachable!("FaceIdentifier::{o:?} is not part of biometric pipeline!")
                    }
                    face_identifier::Output::UpdateConfig => {
                        face_identifier_update_config = Some(());
                    }
                },
                ModelOutput::MegaAgentOne(output) => {
                    match output {
                        mega_agent_one::Output::Config(config) => {
                            mega_agent_one_config = Some(config);
                        }
                        mega_agent_one::Output::Iris(iris::Output::Estimate(
                            iris::EstimateOutput {
                                iris_code,
                                mask_code,
                                iris_code_version,
                                metadata,
                                normalized_image,
                            },
                        )) => {
                            iris_left = Some(EyePipeline {
                                iris_code,
                                mask_code,
                                iris_code_version,
                                metadata,
                                iris_normalized_image: normalized_image,
                            });

                            self.set_timeout();
                            progress += IRIS_ESTIMATE_PROGRESS;
                        }
                        mega_agent_one::Output::Iris(iris::Output::Version(version)) => {
                            iris_version = Some(version);
                        }
                        mega_agent_one::Output::Iris(
                            iris::Output::Error(error),
                            // If IIP or Iris fail, there is not much we can do.
                        ) => return Err(Error::Iris(error))?,
                        mega_agent_one::Output::IRNet(ir_net::Output::Version(version)) => {
                            ir_net_version = Some(version);
                        }
                        o @ mega_agent_one::Output::IRNet(_) => {
                            unreachable!("{o:?} is not part of biometric pipeline!")
                        }
                    }
                }
                ModelOutput::MegaAgentTwo(output) => match output {
                    mega_agent_two::Output::Iris(iris::Output::Estimate(
                        iris::EstimateOutput {
                            iris_code,
                            mask_code,
                            iris_code_version,
                            metadata,
                            normalized_image,
                        },
                    )) => {
                        iris_right = Some(EyePipeline {
                            iris_code,
                            mask_code,
                            iris_code_version,
                            metadata,
                            iris_normalized_image: normalized_image,
                        });

                        self.set_timeout();
                        progress += IRIS_ESTIMATE_PROGRESS;
                    }
                    mega_agent_two::Output::Iris(iris::Output::Version(version)) => {
                        iris_version = Some(version);
                    }
                    mega_agent_two::Output::Iris(
                        iris::Output::Error(error),
                        // If IIP or Iris fail, there is not much we can do.
                    ) => return Err(Error::Iris(error))?,
                    mega_agent_two::Output::Config(config) => {
                        mega_agent_two_config = Some(config);
                    }
                    o @ (mega_agent_two::Output::FaceIdentifier(_)
                    | mega_agent_two::Output::RgbNet(_)
                    | mega_agent_two::Output::FusionRgbNetFaceIdentifier { .. }
                    | mega_agent_two::Output::FusionError(_)) => {
                        unreachable!("{o:?} is not part of biometric pipeline!")
                    }
                },
                ModelOutput::Timeout => {
                    let pending_results: Vec<String> = vec![
                        ("iris_left".to_owned(), iris_left.is_none()),
                        ("iris_right".to_owned(), iris_right.is_none()),
                        ("face_identifier_bundle".to_owned(), face_identifier_bundle.is_none()),
                        (
                            "face_identifier_update_config".to_owned(),
                            face_identifier_update_config.is_none(),
                        ),
                        ("iris_version".to_owned(), iris_version.is_none()),
                        ("ir_net_version".to_owned(), ir_net_version.is_none()),
                        ("mega_agent_one_config".to_owned(), mega_agent_one_config.is_none()),
                        ("mega_agent_two_config".to_owned(), mega_agent_two_config.is_none()),
                    ]
                    .iter()
                    .filter(|(_, b)| *b)
                    .map(|(s, _)| s.clone())
                    .collect();

                    tracing::error!(
                        "Agents that didn't report results at the time of exit: {:?}",
                        pending_results
                    );

                    return Err(Error::Timeout)?;
                }
            }
            orb.led.biometric_pipeline_progress(
                MIN_PROGRESS + progress * (MAX_PROGRESS - MIN_PROGRESS),
            );
        }

        orb.disable_mega_agent_one();
        orb.disable_mega_agent_two();

        tracing::info!("Biometric pipeline <benchmark>: {} ms", now.elapsed().as_millis());
        DATADOG.timing(
            "orb.main.time.signup.biometric_process",
            now.elapsed().as_millis().try_into()?,
            NO_TAGS,
        )?;

        Ok(Pipeline {
            v2: PipelineV2 {
                eye_left: iris_left.unwrap(),
                eye_right: iris_right.unwrap(),
                ir_net_version: ir_net_version.unwrap(),
                iris_version: iris_version.clone().unwrap(),
            },
            face_identifier_bundle: face_identifier_bundle.unwrap(),
            mega_agent_one_config: mega_agent_one_config.unwrap(),
            mega_agent_two_config: mega_agent_two_config.unwrap(),
        })
    }

    async fn run_mega_agent_two(
        &mut self,
        orb: &mut Orb,
        input: mega_agent_two::Input,
    ) -> Result<()> {
        orb.mega_agent_two
            .enabled()
            .unwrap()
            .send(port::Input::new(input))
            .await
            .wrap_err("mega_agent_two_send failed")
    }

    async fn run_mega_agent_one(
        &mut self,
        orb: &mut Orb,
        input: mega_agent_one::Input,
    ) -> Result<()> {
        orb.mega_agent_one
            .enabled()
            .unwrap()
            .send(port::Input::new(input))
            .await
            .wrap_err("mega_agent_one_send failed")
    }

    async fn run_face_identifier(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Run [Face Identifier :: FraudChecks]");

        self.run_mega_agent_two(
            orb,
            mega_agent_two::Input::FaceIdentifier(face_identifier::Input::Estimate {
                frame_left: self.face_left.clone(),
                frame_right: self.face_right.clone(),
                frame_self_custody_candidate: self.face_self_custody_candidate.clone(),
                eyes_landmarks_left: self.eye_landmarks_left,
                eyes_landmarks_right: self.eye_landmarks_right,
                eyes_landmarks_self_custody_candidate: self.eye_landmarks_self_custody_candidate,
                bbox_left: self.face_bbox_left,
                bbox_right: self.face_bbox_right,
                bbox_self_custody_candidate: self.face_bbox_self_custody_candidate,
            }),
        )
        .await
    }

    async fn run_iris_left(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Run [Iris :: estimate] for the left eye");

        self.run_mega_agent_one(
            orb,
            mega_agent_one::Input::Iris(iris::Input::Estimate {
                frame: self.eye_left.clone(),
                left_eye: true,
            }),
        )
        .await
    }

    async fn run_iris_right(&mut self, orb: &mut Orb) -> Result<()> {
        tracing::info!("Run [Iris :: estimate] for the right eye");

        self.run_mega_agent_two(
            orb,
            mega_agent_two::Input::Iris(iris::Input::Estimate {
                frame: self.eye_right.clone(),
                left_eye: false,
            }),
        )
        .await
    }

    fn set_timeout(&mut self) {
        self.timeout = Box::pin(time::sleep(MODEL_TIMEOUT));
    }

    async fn run_update_all_configs(&mut self, orb: &mut Orb) -> Result<()> {
        let face_identifier_model_configs =
            orb.config.lock().await.face_identifier_model_configs.clone();
        self.run_mega_agent_two(
            orb,
            mega_agent_two::Input::FaceIdentifier(face_identifier::Input::UpdateConfig(
                face_identifier_model_configs,
            )),
        )
        .await?;
        Ok(())
    }
}
