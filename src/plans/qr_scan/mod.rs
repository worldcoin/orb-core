//! QR-code scanning.

use crate::{
    agents::{camera, qr_code},
    brokers::{BrokerFlow, Orb, OrbPlan},
    consts::{QR_SCAN_REMINDER, RGB_DEFAULT_HEIGHT, RGB_DEFAULT_WIDTH},
    ext::{broadcast::ReceiverExt, mpsc::SenderExt},
    led, mcu, port, sound,
};
use eyre::Result;
use futures::prelude::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time;
use tokio_stream::wrappers::IntervalStream;

pub mod operator;
pub mod user;
pub mod wifi;

/// QR-code scanning schema.
pub trait Schema: Send + Sized {
    /// Returns the sound to tell the user which kind of QR-code is expected.
    fn sound() -> sound::Type;

    /// Returns the LED schema to show during the scanning.
    fn led() -> led::QrScanSchema;

    /// Tries to parse the QR-code value. Returns `None` if the value doesn't
    /// match the schema.
    fn try_parse(code: &str) -> Option<Self>;
}

/// QR-code scanning plan.
pub struct Plan<S: Schema> {
    reminder: Option<IntervalStream>,
    timeout: Option<Pin<Box<time::Sleep>>>,
    qr_code: Result<S, ScanError>,
    ux_started: bool,
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
        _orb: &mut Orb,
        output: port::Output<qr_code::Agent>,
    ) -> Result<BrokerFlow> {
        let qr_code = output.value.payload;
        self.qr_code = S::try_parse(&qr_code).ok_or(ScanError::Invalid);
        // The underlying library sometimes detects ghost QR codes of a few characters. This
        // prevents a voice to be played in those cases.
        if qr_code.len() > 10 {
            return Ok(BrokerFlow::Break);
        }
        tracing::warn!("Small, potentially ghost, QR code detected, skipping: {:?}", qr_code);
        Ok(BrokerFlow::Continue)
    }

    fn handle_rgb_camera(
        &mut self,
        orb: &mut Orb,
        _output: port::Output<camera::rgb::Sensor>,
    ) -> Result<BrokerFlow> {
        // Ensure RGB net is loaded before we ask for QR codes, to avoid delays
        // between the voice and being ready to scan.
        self.start_ux(orb)?;
        Ok(BrokerFlow::Continue)
    }

    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        while let Poll::Ready(mcu::main::Output::AmbientLight(als)) =
            orb.main_mcu.rx_mut().next_broadcast().poll_unpin(cx)?
        {
            if let Some(qr_code) = orb.qr_code.enabled() {
                qr_code.send_now(port::Input::new(qr_code::Input::Als(als.ambient_light_lux)))?;
            }
        }

        if let Some(timeout) = &mut self.timeout {
            if let Poll::Ready(()) = timeout.poll_unpin(cx) {
                tracing::info!("QR code scanning timed out");
                self.qr_code = Err(ScanError::Timeout);
                return Ok(BrokerFlow::Break);
            }
        }
        if let Some(reminder) = &mut self.reminder {
            while reminder.poll_next_unpin(cx).is_ready() {
                orb.sound.build(S::sound())?.push()?;
            }
        }
        Ok(BrokerFlow::Continue)
    }
}

impl<S: Schema> Plan<S> {
    /// Creates a new QR-code scanning plan.
    #[must_use]
    pub fn new(timeout: Option<Duration>) -> Self {
        let timeout = timeout.map(|timeout| Box::pin(time::sleep(timeout)));
        Self { reminder: None, timeout, qr_code: Err(ScanError::Invalid), ux_started: false }
    }

    /// Runs the QR-code scanning plan.
    pub async fn run(mut self, orb: &mut Orb) -> Result<Result<S, ScanError>> {
        orb.start_rgb_camera().await?;
        orb.enable_qr_code()?;
        orb.set_fisheye(RGB_DEFAULT_WIDTH, RGB_DEFAULT_HEIGHT, true).await?;
        orb.run(&mut self).await?;
        orb.led.qr_scan_completed(S::led());
        let qr_code = self.qr_code;
        orb.disable_qr_code();
        orb.stop_rgb_camera().await?;
        Ok(qr_code)
    }

    fn start_ux(&mut self, orb: &mut Orb) -> Result<()> {
        if !self.ux_started {
            orb.led.qr_scan_start(S::led());
            orb.sound.build(S::sound())?.push()?;
            self.ux_started = true;
            let mut reminder = time::interval(QR_SCAN_REMINDER);
            reminder.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            reminder.reset();
            self.reminder = Some(IntervalStream::new(reminder));
        }
        Ok(())
    }
}
