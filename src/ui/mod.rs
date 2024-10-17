//! UI events forwarding to the [orb-ui service](https://github.com/worldcoin/orb-software/orb-ui) through dbus.

use eyre::Result;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task};
use tokio_stream::wrappers::UnboundedReceiverStream;
use zbus::Connection;

use tracing::warn;

use crate::dbus::SignupStateProxy;

macro_rules! event_enum {
    (
        $(#[$($enum_attrs:tt)*])*
        $vis:vis enum $name:ident {
            $(
                $(#[doc = $doc:expr])?
                #[event_enum(method = $method:ident)]
                $(#[$($event_attrs:tt)*])*
                $event:ident $({$($field:ident: $ty:ty),*$(,)?})?,
            )*
        }
    ) => {
        $(#[$($enum_attrs)*])*
        #[derive(Debug, Deserialize, Serialize)]
        $vis enum $name {
            $(
                $(#[doc = $doc])?
                $(#[$($event_attrs)*])*
                $event $({$($field: $ty,)*})?,
            )*
        }

        /// LED engine interface.
        pub trait Engine: Send + Sync {
            $(
                $(#[doc = $doc])?
                fn $method(&self, $($($field: $ty,)*)?);
            )*

            /// Returns a new handler to the shared queue.
            fn clone(&self) -> Box<dyn Engine>;
        }

        impl Engine for Jetson {
            $(
                $(#[doc = $doc])?
                fn $method(&self, $($($field: $ty,)*)?) {
                    let event = $name::$event $({$($field,)*})?;
                    self.tx.send(event).expect("LED engine is not running");
                }
            )*

            fn clone(&self) -> Box<dyn Engine> {
                Box::new(Jetson { tx: self.tx.clone() })
            }
        }

        impl Engine for Fake {
            $(
                $(#[doc = $doc])?
                #[allow(unused_variables)]
                fn $method(&self, $($($field: $ty,)*)?) {}
            )*

            fn clone(&self) -> Box<dyn Engine> {
                Box::new(Fake)
            }
        }
    };
}

/// QR-code scanning schema.
#[derive(Debug, Deserialize, Serialize)]
pub enum QrScanSchema {
    /// Operator QR-code scanning.
    Operator,
    /// Operator QR-code scanning, self-serve mode.
    OperatorSelfServe,
    /// User QR-code scanning.
    User,
    /// WiFi QR-code scanning.
    Wifi,
}

/// QR-code scanning schema.
#[derive(Debug, Deserialize, Serialize)]
pub enum QrScanUnexpectedReason {
    /// Invalid QR code
    Invalid,
    /// Wrong QR Format
    WrongFormat,
}

/// Signup failure reason
#[derive(Debug, Deserialize, Serialize)]
pub enum SignupFailReason {
    /// Timeout
    Timeout,
    /// Face not found
    FaceNotFound,
    /// User already exists
    Duplicate,
    /// Server error
    Server,
    /// Verification error
    Verification,
    /// Orb software versions are deprecated.
    SoftwareVersionDeprecated,
    /// Orb software versions are outdated.
    SoftwareVersionBlocked,
    /// Upload custody images error
    UploadCustodyImages,
    /// Unknown, unexpected error, or masked signup failure
    Unknown,
}

event_enum! {
    /// Definition of all the events
    #[allow(dead_code)]
    enum Event {
        /// Orb boot up.
        #[event_enum(method = bootup)]
        Bootup,
        /// Orb token was acquired
        #[event_enum(method = boot_complete)]
        BootComplete { api_mode: bool },
        /// Start of QR scan.
        #[event_enum(method = qr_scan_start)]
        QrScanStart {
            schema: QrScanSchema,
        },
        /// QR captured
        #[event_enum(method = qr_scan_capture)]
        QrScanCapture,
        /// QR scan completed.
        #[event_enum(method = qr_scan_completed)]
        QrScanCompleted {
            schema: QrScanSchema,
        },
        /// QR scan succeeded.
        #[event_enum(method = qr_scan_success)]
        QrScanSuccess {
            schema: QrScanSchema,
        },
        /// QR scan is valid but unexpected.
        #[event_enum(method = qr_scan_unexpected)]
        QrScanUnexpected {
            schema: QrScanSchema,
            reason: QrScanUnexpectedReason
        },
        /// QR scan failed.
        #[event_enum(method = qr_scan_fail)]
        QrScanFail {
            schema: QrScanSchema,
        },
        /// QR scan failed due to timeout
        #[event_enum(method = qr_scan_timeout)]
        QrScanTimeout {
            schema: QrScanSchema,
        },
        /// Magic QR action completed
        #[event_enum(method = magic_qr_action_completed)]
        MagicQrActionCompleted {
            success: bool,
        },
        /// Start of the signup phase, triggered on Orb button press. Operator-based signup.
        #[event_enum(method = signup_start_operator)]
        SignupStartOperator,
        /// Start of the capture phase, triggered on button press
        #[event_enum(method = signup_start)]
        SignupStart,
        /// Biometric capture half of the objectives completed.
        #[event_enum(method = biometric_capture_half_objectives_completed)]
        BiometricCaptureHalfObjectivesCompleted,
        /// Biometric capture all of the objectives completed.
        #[event_enum(method = biometric_capture_all_objectives_completed)]
        BiometricCaptureAllObjectivesCompleted,
        /// Biometric capture progress.
        #[event_enum(method = biometric_capture_progress)]
        BiometricCaptureProgress {
            progress: f64,
        },
        /// Biometric capture occlusion.
        #[event_enum(method = biometric_capture_occlusion)]
        BiometricCaptureOcclusion {
            occlusion_detected: bool
        },
        /// User not in distance range.
        #[event_enum(method = biometric_capture_distance)]
        BiometricCaptureDistance {
            in_range: bool
        },
        /// Biometric capture succeeded.
        #[event_enum(method = biometric_capture_success)]
        BiometricCaptureSuccess,
        /// Starting enrollment.
        #[event_enum(method = starting_enrollment)]
        StartingEnrollment,
        /// Biometric pipeline progress.
        #[event_enum(method = biometric_pipeline_progress)]
        BiometricPipelineProgress {
            progress: f64,
        },
        /// Biometric pipeline succeed.
        #[event_enum(method = biometric_pipeline_success)]
        BiometricPipelineSuccess,
        /// Signup success.
        #[event_enum(method = signup_success)]
        SignupSuccess,
        /// Signup failure.
        #[event_enum(method = signup_fail)]
        SignupFail {
            reason: SignupFailReason,
        },
        /// Idle mode.
        #[event_enum(method = idle)]
        Idle,
        /// Orb shutdown.
        #[event_enum(method = shutdown)]
        Shutdown {
            requested: bool,
        },

        /// Network connection successful
        #[event_enum(method = network_connection_success)]
        NetworkConnectionSuccess,
        /// Good internet connection.
        #[event_enum(method = good_internet)]
        GoodInternet,
        /// Slow internet connection.
        #[event_enum(method = slow_internet)]
        SlowInternet,
        /// Slow internet with the intent of starting a signup.
        #[event_enum(method = slow_internet_for_signup)]
        SlowInternetForSignup,
        /// No internet connection.
        #[event_enum(method = no_internet)]
        NoInternet,
        /// No internet with the intent of starting a signup.
        #[event_enum(method = no_internet_for_signup)]
        NoInternetForSignup,
        /// Good wlan connection.
        #[event_enum(method = good_wlan)]
        GoodWlan,
        /// Slow wlan connection.
        #[event_enum(method = slow_wlan)]
        SlowWlan,
        /// No wlan connection.
        #[event_enum(method = no_wlan)]
        NoWlan,

        /// Battery level indicator.
        #[event_enum(method = battery_capacity)]
        BatteryCapacity {
            percentage: u32,
        },
        /// Battery charging indicator.
        #[event_enum(method = battery_is_charging)]
        BatteryIsCharging {
            is_charging: bool,
        },

        /// Pause sending messages to the MCU. LED animations are still computed in the background
        #[event_enum(method = pause)]
        Pause,
        /// Resume sending messages to the MCU.
        #[event_enum(method = resume)]
        Resume,

        /// In recovery image
        #[event_enum(method = recovery)]
        RecoveryImage,

        /// Set volume [0..100]
        #[event_enum(method = sound_volume)]
        SoundVolume {
            level: u64
        },
        /// Set language
        #[event_enum(method = sound_language)]
        SoundLanguage {
            lang: Option<String>,
        },
        /// Plays boot-up complete sound for testing
        #[event_enum(method = sound_test)]
        SoundTest,
    }
}

/// LED engine for the Orb hardware.
pub struct Jetson {
    tx: mpsc::UnboundedSender<Event>,
}

/// LED engine interface which does nothing.
pub struct Fake;

impl Jetson {
    /// Creates the event forwarder
    #[must_use]
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        task::spawn(event_loop(rx));
        Self { tx }
    }
}

#[allow(clippy::too_many_lines)]
async fn event_loop(rx: mpsc::UnboundedReceiver<Event>) -> Result<()> {
    let mut rx = UnboundedReceiverStream::new(rx);
    let connection = Connection::session().await?;
    let proxy = SignupStateProxy::new(&connection).await?;
    loop {
        while let Some(event) = rx.next().await {
            match proxy.orb_signup_state_event(serde_json::to_string(&event)?).await {
                Ok(()) => {}
                Err(e) => {
                    warn!("Error: {:#?}", e);
                }
            }
        }
    }
}
