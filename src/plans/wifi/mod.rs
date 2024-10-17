//! Connect to Wifi.

use crate::{
    brokers::Orb,
    consts::NETWORK_CONNECTION_TIMEOUT,
    network,
    plans::qr_scan,
    ui::{QrScanSchema, QrScanUnexpectedReason},
};
use eyre::{Error, Result};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// WiFi plan.
pub struct Plan;

impl Plan {
    /// Checks whether connected to a WiFi network, if not connected scan the
    /// hotspot QR code.
    pub async fn ensure_network_connection(&self, orb: &mut Orb) -> Result<()> {
        let mut in_progress_start = Instant::now();
        let mut has_requested_qr_code = false;
        let success = |orb: &mut Orb, has_requested_qr_code| {
            if has_requested_qr_code {
                orb.ui.network_connection_success();
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
                        orb.ui.qr_scan_fail(QrScanSchema::Wifi);
                    }
                    has_requested_qr_code = true;
                    match qr_scan::Plan::new(None, false).run(orb).await? {
                        Ok((credentials, _)) => {
                            tracing::info!(
                                "Read WiFi credentials from hotspot QR: {:?}",
                                credentials
                            );
                            network::join(credentials).await?;
                            in_progress_start = Instant::now();
                        }
                        Err(qr_scan::ScanError::Invalid) => {
                            orb.ui.qr_scan_unexpected(
                                QrScanSchema::Wifi,
                                QrScanUnexpectedReason::WrongFormat,
                            );
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
