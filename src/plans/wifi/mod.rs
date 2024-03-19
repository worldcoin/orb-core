//! Connect to Wifi.

use crate::{
    brokers::Orb,
    consts::NETWORK_CONNECTION_TIMEOUT,
    led::QrScanSchema,
    network,
    plans::qr_scan,
    sound::{Melody, Type, Voice},
};
use eyre::{Error, Result};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// WiFi plan.
pub struct Plan {}

impl Plan {
    /// Creates a new WiFi plan.
    #[allow(clippy::new_without_default)]
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// Checks whether connected to a WiFi network, if not connected scan the
    /// hotspot QR code.
    pub async fn ensure_network_connection(&self, orb: &mut Orb) -> Result<()> {
        let mut in_progress_start = Instant::now();
        let mut has_requested_qr_code = false;
        let success = |orb: &mut Orb, has_requested_qr_code| {
            if has_requested_qr_code {
                orb.sound.build(Type::Melody(Melody::QrLoadSuccess))?.push()?;
                orb.led.qr_scan_success(QrScanSchema::Wifi);
            }
            tracing::debug!("Network is connected");
            Ok::<(), Error>(())
        };
        loop {
            match network::status().await? {
                network::Status::Connected { has_internet: true } => {
                    success(orb, has_requested_qr_code)?;
                    break;
                }
                network::Status::InProgress
                | network::Status::Connected { has_internet: false }
                    if in_progress_start.elapsed() < NETWORK_CONNECTION_TIMEOUT =>
                {
                    tracing::debug!("Network connection in progress");
                    sleep(Duration::from_millis(250)).await;
                }
                network::Status::Connected { has_internet: false }
                | network::Status::Disconnected
                | network::Status::InProgress => {
                    tracing::debug!("Network is disconnected, or has no connection to the backend");
                    if has_requested_qr_code {
                        orb.led.qr_scan_fail(QrScanSchema::Wifi);
                    }
                    has_requested_qr_code = true;
                    match qr_scan::Plan::new(None).run(orb).await? {
                        Ok(credentials) => {
                            orb.sound.build(Type::Melody(Melody::QrCodeCapture))?.push()?;
                            tracing::info!(
                                "Read WiFi credentials from hotspot QR: {:?}",
                                credentials
                            );
                            network::join(credentials).await?;
                            in_progress_start = Instant::now();
                        }
                        Err(qr_scan::ScanError::Invalid) => {
                            orb.led.qr_scan_fail(QrScanSchema::Wifi);
                            orb.sound.build(Type::Melody(Melody::SoundError))?.push()?;
                            orb.sound.build(Type::Voice(Voice::WrongQrCodeFormat))?.push()?;
                        }
                        Err(qr_scan::ScanError::Timeout) => {}
                    }
                    orb.reset_rgb_camera().await?;
                }
            }
        }
        Ok(())
    }
}
