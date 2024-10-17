//! QR-code scanning.

pub mod operator;
pub mod user;
pub mod wifi;

use crate::{
    agents::{camera, qr_code},
    brokers::{Orb, OrbPlan},
    consts::{QR_SCAN_REMINDER, RGB_DEFAULT_HEIGHT, RGB_DEFAULT_WIDTH, RGB_FPS, RGB_FPS_REDUCED},
    ext::{broadcast::ReceiverExt as _, mpsc::SenderExt as _},
    mcu, ui,
    ui::QrScanSchema,
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::prelude::*;
use std::{
    mem::replace,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time;
use tokio_stream::wrappers::IntervalStream;

/// QR-code scanning schema.
pub trait Schema: Send + Sized {
    /// Returns the LED schema to show during the scanning.
    fn ui() -> ui::QrScanSchema;

    /// Tries to parse the QR-code value. Returns `None` if the value doesn't
    /// match the schema.
    fn try_parse(code: &str) -> Option<Self>;
}

/// QR-code scanning plan.
pub struct Plan<S: Schema> {
    reminder: Option<IntervalStream>,
    timeout: Option<Pin<Box<time::Sleep>>>,
    fps: u32,
    qr_code: Result<(S, String), ScanError>,
    ux_started: bool,
    self_serve: bool,
}

/// Error returned by the qr-code scannin plan.
#[derive(Debug)]
pub enum ScanError {
    /// QR-code is not valid.
    Invalid,
    /// Scanning timed out.
    Timeout,
}

impl<S: Schema> OrbPlan for Plan<S> {
    fn handle_qr_code(
        &mut self,
        orb: &mut Orb,
        output: port::Output<qr_code::Agent>,
    ) -> Result<BrokerFlow> {
        let qr_code = output.value.payload;
        // The underlying library sometimes detects ghost QR codes of a few characters. This
        // prevents a voice to be played in those cases.
        if qr_code.len() <= 10 {
            tracing::warn!("Small, potentially ghost, QR code detected, skipping: {qr_code:?}");
            return Ok(BrokerFlow::Continue);
        }
        orb.ui.qr_scan_capture();
        self.qr_code =
            S::try_parse(&qr_code).map(|parsed| (parsed, qr_code)).ok_or(ScanError::Invalid);
        Ok(BrokerFlow::Break)
    }

    fn handle_rgb_camera(
        &mut self,
        orb: &mut Orb,
        _output: port::Output<camera::rgb::Sensor>,
    ) -> Result<BrokerFlow> {
        // Ensure RGB net is loaded before we ask for QR codes, to avoid delays
        // between the voice and being ready to scan.
        self.start_ux(orb);
        Ok(BrokerFlow::Continue)
    }

    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        while let Poll::Ready(mcu::main::Output::AmbientLight(als)) =
            orb.main_mcu.rx_mut().next_broadcast().poll_unpin(cx)?
        {
            if let Some(qr_code) = orb.qr_code.enabled() {
                qr_code
                    .tx
                    .send_now(port::Input::new(qr_code::Input::Als(als.ambient_light_lux)))?;
            }
        }

        if let Some(timeout) = &mut self.timeout {
            if let Poll::Ready(()) = timeout.poll_unpin(cx) {
                tracing::info!("QR code scanning timed out");
                self.qr_code = Err(ScanError::Timeout);
                return Ok(BrokerFlow::Break);
            }
        }
        Ok(BrokerFlow::Continue)
    }
}

impl<S: Schema> Plan<S> {
    /// Creates a new QR-code scanning plan.
    #[must_use]
    pub fn new(timeout: Option<Duration>, reduced_fps: bool) -> Self {
        let timeout = timeout.map(|timeout| Box::pin(time::sleep(timeout)));
        let fps = if reduced_fps { RGB_FPS_REDUCED } else { RGB_FPS };
        Self {
            reminder: None,
            timeout,
            fps,
            qr_code: Err(ScanError::Invalid),
            ux_started: false,
            self_serve: false,
        }
    }

    /// Runs the QR-code scanning plan.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Result<(S, String), ScanError>> {
        self.run_pre(orb).await?;
        orb.run(&mut self).await?;
        let qr_code = self.take_qr_code();
        self.run_post(orb).await?;
        Ok(qr_code)
    }

    pub(crate) async fn run_pre(&mut self, orb: &mut Orb) -> Result<()> {
        orb.start_rgb_camera(self.fps).await?;
        orb.enable_qr_code().await?;
        orb.set_fisheye(RGB_DEFAULT_WIDTH, RGB_DEFAULT_HEIGHT, true).await?;
        self.self_serve = orb.config.lock().await.self_serve;
        Ok(())
    }

    pub(crate) async fn run_post(&mut self, orb: &mut Orb) -> Result<()> {
        orb.disable_qr_code();
        orb.stop_rgb_camera().await?;
        Ok(())
    }

    pub(crate) fn take_qr_code(&mut self) -> Result<(S, String), ScanError> {
        replace(&mut self.qr_code, Err(ScanError::Invalid))
    }

    fn start_ux(&mut self, orb: &mut Orb) {
        if !self.ux_started {
            // differentiated ux for operator qr code depending on self-serve mode
            let schema = match S::ui() {
                QrScanSchema::Operator => {
                    if self.self_serve {
                        QrScanSchema::OperatorSelfServe
                    } else {
                        QrScanSchema::Operator
                    }
                }
                x => x,
            };
            orb.ui.qr_scan_start(schema);
            self.ux_started = true;
            let mut reminder = time::interval(QR_SCAN_REMINDER);
            reminder.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            reminder.reset();
            self.reminder = Some(IntervalStream::new(reminder));
        }
    }
}
