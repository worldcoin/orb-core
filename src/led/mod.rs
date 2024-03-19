//! LED engine.

pub mod center;
pub mod operator;
pub mod ring;

use crate::{
    consts::LED_ENGINE_FPS,
    mcu::{
        self,
        main::{OperatorLedsSequence, Rgb, CENTER_LED_COUNT, RING_LED_COUNT},
        Mcu,
    },
    pid::{InstantTimer, Timer},
};
use eyre::Result;
use futures::{future, future::Either, prelude::*};
#[cfg(feature = "ui-test")]
use log::debug;
use std::{any::Any, collections::BTreeMap, f64::consts::PI, time::Duration};
use tokio::{sync::mpsc, task, time};
use tokio_stream::wrappers::{IntervalStream, UnboundedReceiverStream};

#[allow(missing_docs)]
impl Rgb {
    const OFF: Rgb = Rgb(0, 0, 0);
    const OPERATOR_AMBER: Rgb = Rgb(20, 16, 0);
    // To help quickly distinguish dev vs prod software,
    // the default operator LED color is white for prod, yellow for dev
    const OPERATOR_DEFAULT: Rgb = {
        #[cfg(not(feature = "stage"))]
        {
            Rgb(20, 20, 20)
        }
        #[cfg(feature = "stage")]
        {
            Rgb(8, 25, 8)
        }
    };
    const OPERATOR_VERSIONS_DEPRECATED: Rgb = Rgb(128, 128, 0);
    const OPERATOR_VERSIONS_OUTDATED: Rgb = Rgb(255, 0, 0);
    const USER_AMBER: Rgb = Rgb(23, 13, 0);
    const USER_QR_SCAN: Rgb = Rgb(24, 24, 24);
    const USER_RED: Rgb = Rgb(30, 2, 0);
    const USER_SIGNUP: Rgb = Rgb(31, 31, 31);
}

const GAMMA: f64 = 2.5;

const LEVEL_BACKGROUND: u8 = 0;
const LEVEL_FOREGROUND: u8 = 10;
const LEVEL_NOTICE: u8 = 20;

const BIOMETRIC_PIPELINE_MAX_PROGRESS: f64 = 0.875;

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
        #[derive(Debug)]
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
#[derive(Debug)]
pub enum QrScanSchema {
    /// Operator QR-code scanning.
    Operator,
    /// User QR-code scanning.
    User,
    /// WiFi QR-code scanning.
    Wifi,
}

event_enum! {
    #[allow(dead_code)]
    enum Event {
        /// Orb boot up.
        #[event_enum(method = bootup)]
        Bootup,
        /// Orb token was acquired
        #[event_enum(method = boot_complete)]
        BootComplete,
        /// Start of the signup phase, triggered on button press
        #[event_enum(method = signup_start)]
        SignupStart,
        /// Start of QR scan.
        #[event_enum(method = qr_scan_start)]
        QrScanStart {
            schema: QrScanSchema,
        },
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
        },
        /// QR scan failed.
        #[event_enum(method = qr_scan_fail)]
        QrScanFail {
            schema: QrScanSchema,
        },
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
        /// Signup unique.
        #[event_enum(method = signup_unique)]
        SignupSuccess,
        /// Signup failure.
        #[event_enum(method = signup_fail)]
        SignupFail,
        /// Orb software versions are deprecated.
        #[event_enum(method = version_deprecated)]
        SoftwareVersionDeprecated,
        /// Orb software versions are outdated.
        #[event_enum(method = version_blocked)]
        SoftwareVersionBlocked,
        /// Idle mode.
        #[event_enum(method = idle)]
        Idle,
        /// Orb shutdown.
        #[event_enum(method = shutdown)]
        Shutdown {
            requested: bool,
        },

        /// Good internet connection.
        #[event_enum(method = good_internet)]
        GoodInternet,
        /// Slow internet connection.
        #[event_enum(method = slow_internet)]
        SlowInternet,
        /// No internet connection.
        #[event_enum(method = no_internet)]
        NoInternet,
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
    }
}

/// Returned by [`Animation::animate`]
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum AnimationState {
    /// The animation is finished and shouldn't be called again
    Finished,
    /// The animation is still running
    Running,
}

impl AnimationState {
    /// if it is the `Running` variant
    #[must_use]
    pub fn is_running(&self) -> bool {
        *self == AnimationState::Running
    }
}

/// Generic animation.
pub trait Animation: Send + 'static {
    /// Animation frame type.
    type Frame;

    /// Upcasts a reference to self to the dynamic object [`Any`].
    fn as_any(&self) -> &dyn Any;

    /// Upcasts a mutable reference to self to the dynamic object [`Any`].
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Calculates the next animation frame according to the time delta.
    /// Returns [`AnimationState::Finished`] if the animation is finished
    /// and shouldn't be called again.
    fn animate(&mut self, frame: &mut Self::Frame, dt: f64, idle: bool) -> AnimationState;

    /// Sets a transition effect from the previous animation to this animation.
    fn transition_from(&mut self, _superseded: &dyn Any) {}

    /// Signals the animation to stop. It shouldn't necessarily stop
    /// immediately.
    fn stop(&mut self) {}
}

/// LED engine for the Orb hardware.
pub struct Jetson {
    tx: mpsc::UnboundedSender<Event>,
}

/// LED engine interface which does nothing.
pub struct Fake;

/// Frame for the front LED ring.
pub type RingFrame = [Rgb; RING_LED_COUNT];

/// Frame for the center LEDs.
pub type CenterFrame = [Rgb; CENTER_LED_COUNT];

type DynamicAnimation<Frame> = Box<dyn Animation<Frame = Frame>>;

struct Runner {
    main_mcu: Box<dyn Mcu<mcu::Main>>,
    timer: InstantTimer,
    ring_animations_stack: AnimationsStack<RingFrame>,
    center_animations_stack: AnimationsStack<CenterFrame>,
    ring_frame: RingFrame,
    center_frame: CenterFrame,
    operator_frame: OperatorLedsSequence,
    operator_connection: operator::Connection,
    operator_battery: operator::Battery,
    operator_blink: operator::Blink,
    operator_pulse: operator::Pulse,
    operator_action: operator::Bar,
    operator_signup_phase: operator::SignupPhase,
    paused: bool,
}

struct AnimationsStack<Frame: 'static> {
    stack: BTreeMap<u8, RunningAnimation<Frame>>,
}

struct RunningAnimation<Frame> {
    animation: DynamicAnimation<Frame>,
    kill: bool,
}

impl Jetson {
    /// Creates a new LED engine.
    #[must_use]
    pub fn spawn(main_mcu: Box<dyn Mcu<mcu::Main>>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        task::spawn(event_loop(main_mcu, rx));
        Self { tx }
    }
}

#[allow(clippy::too_many_lines)]
async fn event_loop(
    main_mcu: Box<dyn Mcu<mcu::Main>>,
    rx: mpsc::UnboundedReceiver<Event>,
) -> Result<()> {
    let mut interval = time::interval(Duration::from_millis(1000 / LED_ENGINE_FPS));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut interval = IntervalStream::new(interval);
    let mut rx = UnboundedReceiverStream::new(rx);
    let mut runner = Runner::new(main_mcu);
    loop {
        match future::select(rx.next(), interval.next()).await {
            Either::Left((None, _)) => {
                break;
            }
            Either::Left((Some(event), _)) => {
                runner.event(&event);
            }
            Either::Right(_) => {
                runner.run().await?;
            }
        }
    }
    Ok(())
}

impl Runner {
    fn new(main_mcu: Box<dyn Mcu<mcu::Main>>) -> Self {
        Self {
            main_mcu,
            timer: InstantTimer::default(),
            ring_animations_stack: AnimationsStack::new(),
            center_animations_stack: AnimationsStack::new(),
            ring_frame: [Rgb(0, 0, 0); RING_LED_COUNT],
            center_frame: [Rgb(0, 0, 0); CENTER_LED_COUNT],
            operator_frame: OperatorLedsSequence::default(),
            operator_connection: operator::Connection::default(),
            operator_battery: operator::Battery::default(),
            operator_blink: operator::Blink::default(),
            operator_pulse: operator::Pulse::default(),
            operator_action: operator::Bar::default(),
            operator_signup_phase: operator::SignupPhase::default(),
            paused: false,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn event(&mut self, event: &Event) {
        #[cfg(feature = "ui-test")]
        tracing::debug!("LED event: {:?}", event);

        match event {
            Event::Bootup => {
                self.stop_ring(LEVEL_NOTICE, true);
                self.stop_center(LEVEL_NOTICE, true);
                self.set_ring(LEVEL_BACKGROUND, ring::Idle::default());
                self.operator_pulse.trigger(2048.0, 1., 1., false);
            }
            Event::BootComplete => self.operator_pulse.stop(),
            Event::Shutdown { requested } => {
                self.set_center(
                    LEVEL_NOTICE,
                    center::Alert::new(
                        if *requested { Rgb::USER_QR_SCAN } else { Rgb::USER_AMBER },
                        vec![0.0, 0.3, 0.45, 0.3, 0.45, 0.45],
                        false,
                    ),
                );
                self.operator_action.trigger(1.0, Rgb::OFF, true, false, true);
            }
            Event::SignupStart => {
                // starting signup sequence, operator LEDs in blue
                // animate from left to right (`operator_action`)
                // and then keep first LED on as a background (`operator_signup_phase`)
                self.operator_action.trigger(0.6, Rgb::OPERATOR_DEFAULT, false, true, false);
                self.operator_signup_phase.signup_phase_started();

                // clear user animations
                self.stop_ring(LEVEL_FOREGROUND, true);
                self.stop_center(LEVEL_FOREGROUND, true);
                self.stop_ring(LEVEL_NOTICE, true);
                self.stop_center(LEVEL_NOTICE, true);
            }
            Event::QrScanStart { schema } => {
                self.set_center(
                    LEVEL_FOREGROUND,
                    center::Wave::new(Rgb::USER_QR_SCAN, 5.0, 0.5, true),
                );

                match schema {
                    QrScanSchema::Operator => {
                        self.operator_signup_phase.operator_qr_code_ok();
                    }
                    QrScanSchema::Wifi => {
                        self.operator_connection.no_wlan();
                    }
                    QrScanSchema::User => {
                        self.operator_signup_phase.user_qr_code_ok();
                        // initialize ring with short segment to invite user to scan QR
                        self.set_ring(LEVEL_FOREGROUND, ring::Slider::new(0.0, Rgb::USER_SIGNUP));
                    }
                };
            }
            Event::QrScanCompleted { schema: _ } => {
                self.set_center(
                    LEVEL_NOTICE,
                    center::Alert::new(Rgb::USER_QR_SCAN, vec![0.0, 0.3, 0.45, 0.46], false),
                );
                self.stop_center(LEVEL_FOREGROUND, true);
            }
            Event::QrScanUnexpected { schema } => {
                match schema {
                    QrScanSchema::User => {
                        self.operator_signup_phase.user_qr_code_issue();
                    }
                    QrScanSchema::Operator => {
                        self.operator_signup_phase.operator_qr_code_issue();
                    }
                    QrScanSchema::Wifi => {}
                }
                self.stop_center(LEVEL_FOREGROUND, true);
            }
            Event::QrScanFail { schema } => {
                match schema {
                    QrScanSchema::User | QrScanSchema::Operator => {
                        self.stop_center(LEVEL_FOREGROUND, true);
                        self.set_center(LEVEL_FOREGROUND, center::Static::new(Rgb::OFF, None));
                        self.operator_signup_phase.failure();
                    }
                    QrScanSchema::Wifi => {}
                }
                self.stop_ring(LEVEL_FOREGROUND, true);
            }
            Event::QrScanSuccess { schema } => {
                if matches!(schema, QrScanSchema::Operator) {
                    self.operator_signup_phase.operator_qr_captured();
                } else if matches!(schema, QrScanSchema::User) {
                    self.operator_signup_phase.user_qr_captured();
                    // initialize ring with short segment to invite user to start iris capture
                    self.set_ring(
                        LEVEL_NOTICE,
                        ring::Slider::new(0.0, Rgb::USER_SIGNUP).pulse_remaining(),
                    );
                    // off background for biometric-capture, which relies on LEVEL_NOTICE animations
                    self.stop_center(LEVEL_FOREGROUND, true);
                    self.set_center(LEVEL_FOREGROUND, center::Static::new(Rgb::OFF, None));
                }
                self.stop_ring(LEVEL_FOREGROUND, true);
            }
            Event::BiometricCaptureHalfObjectivesCompleted => {
                // do nothing
            }
            Event::BiometricCaptureAllObjectivesCompleted => {
                self.operator_signup_phase.irises_captured();
            }
            Event::BiometricCaptureProgress { progress } => {
                if self
                    .ring_animations_stack
                    .stack
                    .get_mut(&LEVEL_NOTICE)
                    .and_then(|RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Slider>()
                    })
                    .is_none()
                {
                    // in case animation not yet initialized through user QR scan success event
                    // initialize ring with short segment to invite user to start iris capture
                    self.set_ring(
                        LEVEL_NOTICE,
                        ring::Slider::new(0.0, Rgb::USER_SIGNUP).pulse_remaining(),
                    );
                }
                let ring_progress =
                    self.ring_animations_stack.stack.get_mut(&LEVEL_NOTICE).and_then(
                        |RunningAnimation { animation, .. }| {
                            animation.as_any_mut().downcast_mut::<ring::Slider>()
                        },
                    );
                if let Some(ring_progress) = ring_progress {
                    ring_progress.set_progress(*progress, true);
                }
            }
            Event::BiometricCaptureOcclusion { occlusion_detected } => {
                if *occlusion_detected {
                    self.operator_signup_phase.capture_occlusion_issue();
                } else {
                    self.operator_signup_phase.capture_occlusion_ok();
                }
            }
            Event::BiometricCaptureDistance { in_range } => {
                if *in_range {
                    self.operator_signup_phase.capture_distance_ok();
                } else {
                    self.operator_signup_phase.capture_distance_issue();
                }
            }
            Event::BiometricCaptureSuccess => {
                // set ring to full circle based on previous progress animation
                // ring will be reset when biometric pipeline starts showing progress
                let _ = self
                    .ring_animations_stack
                    .stack
                    .get_mut(&LEVEL_NOTICE)
                    .and_then(|RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Slider>()
                    })
                    .map(|x| {
                        x.set_progress(1.0, false);
                    });
                self.stop_center(LEVEL_NOTICE, true);
                self.operator_signup_phase.iris_scan_complete();
            }
            Event::BiometricPipelineProgress { progress } => {
                let ring_animation =
                    self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                        |RunningAnimation { animation, .. }| {
                            animation.as_any_mut().downcast_mut::<ring::Progress>()
                        },
                    );
                if let Some(ring_animation) = ring_animation {
                    ring_animation.set_progress(*progress * BIOMETRIC_PIPELINE_MAX_PROGRESS, None);
                } else {
                    self.set_ring(
                        LEVEL_FOREGROUND,
                        ring::Progress::new(0.0, None, Rgb::USER_SIGNUP),
                    );
                }

                // operator LED to show pipeline progress
                if *progress <= 0.5 {
                    self.operator_signup_phase.processing_1();
                } else {
                    self.operator_signup_phase.processing_2();
                }
            }
            Event::StartingEnrollment => {
                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_pulse_angle(PI / 180.0 * 20.0);
                }
                self.operator_signup_phase.uploading();
            }
            Event::BiometricPipelineSuccess => {
                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_progress(BIOMETRIC_PIPELINE_MAX_PROGRESS, None);
                }
                self.operator_signup_phase.biometric_pipeline_successful();
            }
            Event::SignupFail => {
                self.operator_signup_phase.failure();

                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_progress(2.0, None);
                }
                self.stop_ring(LEVEL_FOREGROUND, false);
                self.stop_ring(LEVEL_NOTICE, true);
                self.stop_center(LEVEL_NOTICE, true);
            }
            Event::SignupSuccess => {
                self.operator_signup_phase.signup_successful();

                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_progress(2.0, None);
                }
                self.stop_ring(LEVEL_FOREGROUND, false);

                self.stop_ring(LEVEL_NOTICE, true);
                self.stop_center(LEVEL_NOTICE, true);
                self.set_ring(LEVEL_FOREGROUND, ring::Idle::new(Some(Rgb::USER_SIGNUP), Some(3.0)));
            }
            Event::SoftwareVersionDeprecated => {
                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_progress(2.0, None);
                }
                self.stop_ring(LEVEL_FOREGROUND, false);
                self.operator_blink
                    .trigger(Rgb::OPERATOR_VERSIONS_DEPRECATED, vec![0.4, 0.4, 0.4, 0.4, 0.4, 0.4]);
            }
            Event::SoftwareVersionBlocked => {
                let slider = self.ring_animations_stack.stack.get_mut(&LEVEL_FOREGROUND).and_then(
                    |RunningAnimation { animation, .. }| {
                        animation.as_any_mut().downcast_mut::<ring::Progress>()
                    },
                );
                if let Some(slider) = slider {
                    slider.set_progress(2.0, None);
                }
                self.stop_ring(LEVEL_FOREGROUND, false);
                self.operator_blink
                    .trigger(Rgb::OPERATOR_VERSIONS_OUTDATED, vec![0.4, 0.4, 0.4, 0.4, 0.4, 0.4]);
            }
            Event::Idle => {
                self.stop_ring(LEVEL_FOREGROUND, false);
                self.stop_center(LEVEL_FOREGROUND, false);
                self.operator_signup_phase.idle();
            }
            Event::GoodInternet => {
                self.operator_connection.good_internet();
            }
            Event::SlowInternet => {
                self.operator_connection.slow_internet();
            }
            Event::NoInternet => {
                self.operator_connection.no_internet();
            }
            Event::GoodWlan => {
                self.operator_connection.good_wlan();
            }
            Event::SlowWlan => {
                self.operator_connection.slow_wlan();
            }
            Event::NoWlan => {
                self.operator_connection.no_wlan();
            }
            Event::BatteryCapacity { percentage } => {
                self.operator_battery.capacity(*percentage);
            }
            Event::BatteryIsCharging { is_charging } => {
                self.operator_battery.set_charging(*is_charging);
            }
            Event::Pause => {
                self.paused = true;
            }
            Event::Resume => {
                self.paused = false;
            }
            Event::RecoveryImage => {
                self.set_ring(LEVEL_NOTICE, ring::Spinner::triple(Rgb::USER_RED));
            }
        }
    }

    async fn run(&mut self) -> Result<()> {
        let dt = self.timer.get_dt().unwrap_or(0.0);
        self.center_animations_stack.run(&mut self.center_frame, dt);
        if !self.paused {
            self.main_mcu.send_uart(mcu::main::Input::CenterLeds(self.center_frame))?;
        }

        self.operator_battery.animate(&mut self.operator_frame, dt, false);
        self.operator_connection.animate(&mut self.operator_frame, dt, false);
        self.operator_signup_phase.animate(&mut self.operator_frame, dt, false);
        self.operator_blink.animate(&mut self.operator_frame, dt, false);
        self.operator_pulse.animate(&mut self.operator_frame, dt, false);
        self.operator_action.animate(&mut self.operator_frame, dt, false);
        time::sleep(Duration::from_millis(2)).await;
        if !self.paused {
            self.main_mcu.send_uart(mcu::main::Input::OperatorLeds(self.operator_frame))?;
        }

        self.ring_animations_stack.run(&mut self.ring_frame, dt);
        time::sleep(Duration::from_millis(2)).await;
        if !self.paused {
            self.main_mcu.send_uart(mcu::main::Input::RingLeds(self.ring_frame.into()))?;
        }
        Ok(())
    }

    fn set_ring(&mut self, level: u8, animation: impl Animation<Frame = RingFrame>) {
        self.ring_animations_stack.set(level, Box::new(animation));
    }

    fn set_center(&mut self, level: u8, animation: impl Animation<Frame = CenterFrame>) {
        self.center_animations_stack.set(level, Box::new(animation));
    }

    fn stop_ring(&mut self, level: u8, force: bool) {
        self.ring_animations_stack.stop(level, force);
    }

    fn stop_center(&mut self, level: u8, force: bool) {
        self.center_animations_stack.stop(level, force);
    }
}

impl<Frame: 'static> AnimationsStack<Frame> {
    fn new() -> Self {
        Self { stack: BTreeMap::new() }
    }

    fn stop(&mut self, level: u8, force: bool) {
        if let Some(RunningAnimation { animation, kill }) = self.stack.get_mut(&level) {
            animation.stop();
            *kill = *kill || force;
        }
    }

    fn set(&mut self, level: u8, mut animation: DynamicAnimation<Frame>) {
        if let Some(&top_level) = self.stack.keys().next_back() {
            if top_level <= level {
                let RunningAnimation { animation: superseded, .. } =
                    self.stack.get(&level).or_else(|| self.stack.values().next_back()).unwrap();
                animation.transition_from(superseded.as_any());
            }
        }
        self.stack.insert(level, RunningAnimation { animation, kill: false });
    }

    fn run(&mut self, frame: &mut Frame, dt: f64) {
        let mut top_level = None;
        // Running the top animation.
        let mut completed_animation: Option<DynamicAnimation<Frame>> = None;
        while let Some((&level, RunningAnimation { animation, kill })) =
            self.stack.iter_mut().next_back()
        {
            top_level = Some(level);
            if let Some(completed_animation) = &completed_animation {
                animation.transition_from(completed_animation.as_any());
            }
            if !*kill && animation.animate(frame, dt, false).is_running() {
                break;
            }
            let RunningAnimation { animation, .. } = self.stack.remove(&level).unwrap();
            if completed_animation.is_none() {
                completed_animation = Some(animation);
            }
        }
        // Idling the background animations.
        if let Some(top_level) = top_level {
            self.stack.retain(|&level, RunningAnimation { animation, .. }| {
                if level == top_level {
                    true
                } else {
                    animation.animate(frame, dt, true).is_running()
                }
            });
        }
    }
}
