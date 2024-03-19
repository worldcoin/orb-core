//! Distance measurement agent.

use crate::{
    agents::python,
    consts::{IRIS_SHARPNESS_MIN, IR_FOCUS_DISTANCE, IR_FOCUS_RANGE, IR_FOCUS_RANGE_SMALL},
    led,
    logger::{DATADOG, NO_TAGS},
    port,
    port::Port,
    sound::{self, Melody},
};
use async_trait::async_trait;
use eyre::Result;
use futures::{future::Either, prelude::*, ready};

use orb_sound::SoundFuture;
use std::{
    convert::Infallible,
    pin::Pin,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, SystemTime},
};

/// Distance measurement agent.
///
/// See [the module-level documentation](self) for details.
pub struct Agent {
    /// Sound queue.
    pub sound: Box<dyn sound::Player>,
    /// LED engine.
    pub led: Box<dyn led::Engine>,
}

/// Agent input.
#[derive(Debug)]
pub enum Input {
    /// IR Net estimation.
    IrNetEstimate(python::ir_net::EstimateOutput),
    /// RGB Net estimation.
    RgbNetEstimate(python::rgb_net::EstimateOutput),
    /// Resets the internal state of the agent.
    Reset,
}

const UNKNOWN: u8 = 0;
const IN_RANGE: u8 = 1;
const TOO_CLOSE: u8 = 2;
const TOO_FAR: u8 = 3;

struct Sounds<'a> {
    sound: Box<dyn sound::Player>,
    sound_fut: Option<SoundFuture>,
    state: &'a AtomicU8,
    in_range_index: u8, // 4 bits (msb) for sound number, 4 bits (lsb) for inner sound index
}

impl<'a> Sounds<'a> {
    fn new(sound: Box<dyn sound::Player>, state: &'a AtomicU8) -> Self {
        Self { sound, sound_fut: None, state, in_range_index: 0_u8 }
    }
}

impl Future for Sounds<'_> {
    type Output = Infallible;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(fut) = &mut self.sound_fut {
            ready!(fut.poll_unpin(cx));
        }
        // play sounds following the loop order:
        // 01: first time or when not in range
        // then 02 & 03 interchangeably: when in range
        let sound = match self.state.load(Ordering::Relaxed) {
            IN_RANGE => match self.in_range_index >> 4 & 0x0F {
                0 => match self.in_range_index & 0xF {
                    0 => {
                        self.in_range_index += 1;
                        Some(Melody::IrisScanningLoop01A)
                    }
                    1 => {
                        self.in_range_index += 1;
                        Some(Melody::IrisScanningLoop01B)
                    }
                    2 => {
                        self.in_range_index &= 0xF0;
                        self.in_range_index += 0x10;
                        Some(Melody::IrisScanningLoop01C)
                    }
                    _ => unreachable!(),
                },
                1 => match self.in_range_index & 0xF {
                    0 => {
                        self.in_range_index += 1;
                        Some(Melody::IrisScanningLoop02A)
                    }
                    1 => {
                        self.in_range_index += 1;
                        Some(Melody::IrisScanningLoop02B)
                    }
                    2 => {
                        self.in_range_index &= 0xF0;
                        self.in_range_index += 0x10;
                        Some(Melody::IrisScanningLoop02C)
                    }
                    _ => unreachable!(),
                },
                2 => {
                    match self.in_range_index & 0xF {
                        0 => {
                            self.in_range_index += 1;
                            Some(Melody::IrisScanningLoop03A)
                        }
                        1 => {
                            self.in_range_index += 1;
                            Some(Melody::IrisScanningLoop03B)
                        }
                        2 => {
                            self.in_range_index = 0x10; // loop through loop02 & 03
                            Some(Melody::IrisScanningLoop03C)
                        }
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            },
            UNKNOWN | TOO_CLOSE | TOO_FAR => {
                self.in_range_index &= 0xF0;
                None
            }
            _ => unreachable!(),
        };

        if let Some(sound) = sound {
            self.sound_fut = Some(
                self.sound
                    .build(sound::Type::Melody(sound))
                    .unwrap()
                    .max_delay(Duration::from_millis(100))
                    .push()
                    .unwrap(),
            );
        } else {
            self.sound_fut = None;
        }

        Poll::Pending
    }
}

impl Port for Agent {
    type Input = Input;
    type Output = Infallible;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Agent {
    const NAME: &'static str = "distance";
}

#[async_trait]
impl super::AgentTask for Agent {
    async fn run(self, mut port: port::Inner<Self>) -> Result<()> {
        'reset: loop {
            let mut focus_range = IR_FOCUS_RANGE_SMALL;
            let mut sharp_iris_detected = false;
            let state = Arc::new(AtomicU8::new(UNKNOWN));
            let mut user_came_in_range = false;
            let mut rgb_net_first_distance_date = None;
            let mut sounds = Sounds::new(self.sound.clone(), &state);
            loop {
                match future::select(port.next(), &mut sounds).await {
                    Either::Left((Some(input), _)) => match input.value {
                        Input::IrNetEstimate(ref ir_net_estimate) => {
                            let python::ir_net::EstimateOutput { sharpness, .. } = *ir_net_estimate;
                            if sharpness > IRIS_SHARPNESS_MIN && !sharp_iris_detected {
                                DATADOG.incr(
                                    "orb.main.count.signup.during.biometric_capture.\
                                     sharp_iris_detected",
                                    NO_TAGS,
                                )?;
                                sharp_iris_detected = true;
                            }
                        }
                        Input::RgbNetEstimate(rgb_net_estimate) => {
                            if rgb_net_first_distance_date.is_none() {
                                rgb_net_first_distance_date = Some(SystemTime::now());
                            }
                            let Some(user_distance) = rgb_net_estimate
                                .primary()
                                .map(python::rgb_net::EstimatePredictionOutput::user_distance)
                            else {
                                continue;
                            };
                            let new_state = if focus_range.contains(&user_distance) {
                                focus_range = IR_FOCUS_RANGE;
                                user_came_in_range = true;
                                rgb_net_first_distance_date = Some(SystemTime::now());
                                self.led.biometric_capture_distance(true);
                                IN_RANGE
                            } else {
                                // show "user not in range" only if user was in range before
                                let time_out_of_range = rgb_net_first_distance_date
                                    .unwrap_or(SystemTime::now())
                                    .elapsed()
                                    .unwrap_or(Duration::from_secs(0));
                                if user_came_in_range || time_out_of_range.as_millis() > 2000 {
                                    self.led.biometric_capture_distance(false);
                                }
                                focus_range = IR_FOCUS_RANGE_SMALL;
                                if user_distance < IR_FOCUS_DISTANCE { TOO_CLOSE } else { TOO_FAR }
                            };
                            state.store(new_state, Ordering::Relaxed);
                        }
                        Input::Reset => {
                            tracing::debug!("RESETTING DISTANCE AGENT");
                            continue 'reset;
                        }
                    },
                    Either::Left((None, _)) => return Ok(()),
                    Either::Right((x, _)) => match x {},
                }
            }
        }
    }
}
