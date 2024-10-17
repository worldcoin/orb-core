//! Run background tasks until the button is pressed.

use super::qr_scan;
use crate::{
    agents::{camera, qr_code},
    brokers::{Orb, OrbPlan},
    consts::BUTTON_LONG_PRESS_DURATION,
    ext::broadcast::ReceiverExt as _,
    mcu,
};
use agentwire::{port, BrokerFlow};
use eyre::Result;
use futures::{future::Fuse, prelude::*};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::time;

#[cfg(feature = "internal-data-acquisition")]
use once_cell::sync::Lazy;

#[cfg(feature = "internal-data-acquisition")]
static IMAGE_UPLOAD_DELAY: Lazy<Duration> = Lazy::new(|| {
    Duration::from_secs(
        std::env::var("IMAGE_UPLOAD_DELAY")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(crate::consts::DEFAULT_IMAGE_UPLOAD_DELAY),
    )
});

/// Idle plan.
pub struct Plan {
    user_qr_scan: Option<qr_scan::Plan<qr_scan::user::Data>>,
    is_pressed: bool,
    press_time: Option<SystemTime>,
    ui_idle_delay: Option<Pin<Box<time::Sleep>>>,
    timeout: Fuse<Pin<Box<time::Sleep>>>,
    timed_out: bool,
    #[cfg(feature = "internal-data-acquisition")]
    data_acquisition: bool,
}

/// Idle plan return value.
pub enum Value {
    /// A user QR code was scanned.
    UserQrCode(Result<(qr_scan::user::Data, String), qr_scan::ScanError>),
    /// The button was pressed.
    ButtonPress,
    /// The plan timed out.
    TimedOut,
}

impl OrbPlan for Plan {
    fn handle_qr_code(
        &mut self,
        orb: &mut Orb,
        output: port::Output<qr_code::Agent>,
    ) -> Result<BrokerFlow> {
        if let Some(qr_scan) = &mut self.user_qr_scan {
            qr_scan.handle_qr_code(orb, output)
        } else {
            Ok(BrokerFlow::Continue)
        }
    }

    fn handle_rgb_camera(
        &mut self,
        orb: &mut Orb,
        output: port::Output<camera::rgb::Sensor>,
    ) -> Result<BrokerFlow> {
        if let Some(qr_scan) = &mut self.user_qr_scan {
            qr_scan.handle_rgb_camera(orb, output)
        } else {
            Ok(BrokerFlow::Continue)
        }
    }

    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        if let Some(qr_scan) = &mut self.user_qr_scan {
            if let BrokerFlow::Break = qr_scan.poll_extra(orb, cx)? {
                return Ok(BrokerFlow::Break);
            }
        }
        while let Poll::Ready(output) = orb.main_mcu.rx_mut().next_broadcast().poll_unpin(cx) {
            match output? {
                // Detect short button press to start a new signup.
                mcu::main::Output::Button(true) => {
                    self.is_pressed = true;
                    self.press_time = Some(SystemTime::now());
                }
                mcu::main::Output::Button(false) => {
                    if self.is_pressed
                        && SystemTime::now()
                            .duration_since(self.press_time.unwrap_or(SystemTime::UNIX_EPOCH))
                            .unwrap()
                            <= BUTTON_LONG_PRESS_DURATION
                    {
                        self.is_pressed = false;
                        if self.user_qr_scan.is_none() {
                            return Ok(BrokerFlow::Break);
                        }
                    }
                }
                _ => {}
            }
        }
        if self.user_qr_scan.is_none() {
            if let Some(ui_idle_delay) = &mut self.ui_idle_delay {
                if let Poll::Ready(()) = ui_idle_delay.poll_unpin(cx) {
                    self.ui_idle_delay = None;
                    orb.ui.idle();
                }
            }
        }
        if let Poll::Ready(()) = self.timeout.poll_unpin(cx) {
            self.timed_out = true;
            return Ok(BrokerFlow::Break);
        }
        Ok(BrokerFlow::Continue)
    }
}

impl Plan {
    /// Creates a new Idle plan, which ends when the button is pressed.
    #[must_use]
    pub fn new(
        ui_idle_delay: Option<time::Sleep>,
        #[cfg(feature = "internal-data-acquisition")] data_acquisition: bool,
    ) -> Self {
        Self {
            user_qr_scan: None,
            is_pressed: false,
            press_time: None,
            ui_idle_delay: ui_idle_delay.map(Box::pin),
            timeout: Fuse::terminated(),
            timed_out: false,
            #[cfg(feature = "internal-data-acquisition")]
            data_acquisition,
        }
    }

    /// Creates a new Idle plan, which continuously scans user QR codes in the
    /// background.
    #[must_use]
    pub fn with_user_qr_scan(
        ui_idle_delay: Option<time::Sleep>,
        timeout: Option<Duration>,
        #[cfg(feature = "internal-data-acquisition")] data_acquisition: bool,
    ) -> Self {
        Self {
            user_qr_scan: Some(qr_scan::Plan::new(None, true)),
            is_pressed: false,
            press_time: None,
            ui_idle_delay: ui_idle_delay.map(Box::pin),
            timeout: timeout
                .map_or_else(Fuse::terminated, |timeout| Box::pin(time::sleep(timeout)).fuse()),
            timed_out: false,
            #[cfg(feature = "internal-data-acquisition")]
            data_acquisition,
        }
    }

    /// Runs the idle plan. Returns `Ok(Some(qr_scan_result))` or `Ok(None)`
    /// depending on whether the user QR-code scanning feature was enabled or not.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<Value> {
        if self.ui_idle_delay.is_none() && self.user_qr_scan.is_none() {
            orb.ui.idle();
        }
        #[cfg(feature = "internal-data-acquisition")]
        if self.data_acquisition {
            orb.enable_image_uploader()?;
            orb.image_uploader
                .enabled()
                .unwrap()
                .send(port::Input::new(crate::agents::image_uploader::Input::StartUpload {
                    image_upload_delay: *IMAGE_UPLOAD_DELAY,
                }))
                .await?;
        }

        orb.main_mcu.rx_mut().clear()?;
        if let Some(qr_scan) = &mut self.user_qr_scan {
            qr_scan.run_pre(orb).await?;
        }
        orb.run(self).await?;
        let user_qr_code = self.user_qr_scan.as_mut().map(qr_scan::Plan::take_qr_code);
        if let Some(qr_scan) = &mut self.user_qr_scan {
            qr_scan.run_post(orb).await?;
        }

        #[cfg(feature = "internal-data-acquisition")]
        if self.data_acquisition {
            orb.image_uploader
                .enabled()
                .unwrap()
                .send(port::Input::new(crate::agents::image_uploader::Input::PauseUpload))
                .await?;
            orb.disable_image_uploader();
        }

        if self.ui_idle_delay.is_some() && self.user_qr_scan.is_none() {
            orb.ui.idle();
        }
        if self.timed_out {
            Ok(Value::TimedOut)
        } else if let Some(user_qr_code) = user_qr_code {
            Ok(Value::UserQrCode(user_qr_code))
        } else {
            Ok(Value::ButtonPress)
        }
    }
}
