//! CAN MCU interface.

use super::{Interface, ResultSender};
use crate::utils::spawn_named_thread;
use eyre::{bail, Error, Result};
use futures::{
    channel::mpsc,
    future::{self, Either},
    prelude::*,
};
use libc::CAN_EFF_FLAG;
use nmea_parser::NmeaParser;
use orb_can::fd;
use orb_messages;
use prost::Message;
use std::{marker::PhantomData, time::Duration};
use tokio::{sync::broadcast, task, time::timeout};

const ASYNC_TX_CAPACITY: usize = 100;
const ASYNC_RX_CAPACITY: usize = 100;
const ACK_CAPACITY: usize = 100;
const CAN_SOCKET: &str = "can0";
const CAN_FD_ADDR_JETSON: u32 = 0x80 | CAN_EFF_FLAG;
const TIMEOUT: Duration = Duration::from_millis(300);

/// CAN interface.
pub struct Can<I: Interface>(PhantomData<I>);

/// Create a unique ack number
///
/// - prefix with process ID
/// - append counter
///
/// this added piece of information in the ack number is not strictly necessary
/// but helps filter out acks that are not for us (e.g. acks for other processes)
#[inline]
fn create_ack(counter: u16) -> u32 {
    std::process::id() << 16 | u32::from(counter)
}

/// Check that ack contains the process ID
#[inline]
fn is_ack_for_us(ack_number: u32) -> bool {
    ack_number >> 16 == std::process::id()
}

impl<I: Interface> Can<I> {
    /// Spawns a new CAN interface.
    pub fn spawn(
        input_rx: mpsc::Receiver<(I::Input, Option<ResultSender>)>,
        output_tx: broadcast::Sender<I::Output>,
    ) -> Result<()> {
        let (tx, rx) = fd::open(CAN_SOCKET)?;
        let tx = Self::async_tx(tx);
        let rx = Self::async_rx(rx);
        let (ack_tx, ack_rx) = mpsc::channel(ACK_CAPACITY);
        task::spawn(async move {
            let input_fut = Self::handle_input(tx, input_rx, ack_rx, output_tx.clone());
            let output_fut = Self::handle_output(rx, output_tx, ack_tx);
            match future::try_join(input_fut, output_fut).await {
                Ok(((), ())) => {}
                Err(err) => {
                    tracing::error!("MCU task failed: {:?}", err);
                }
            }
            Ok::<(), Error>(())
        });
        Ok(())
    }

    async fn handle_input(
        mcu_tx: tokio::sync::mpsc::Sender<orb_messages::mcu_main::mcu_message::Message>,
        mut input_rx: mpsc::Receiver<(I::Input, Option<ResultSender>)>,
        mut ack_rx: mpsc::Receiver<orb_messages::mcu_main::Ack>,
        output_tx: broadcast::Sender<I::Output>,
    ) -> Result<()> {
        let mut counter: u16 = 0;
        loop {
            match future::select(input_rx.next(), ack_rx.next()).await {
                Either::Left((None, _)) | Either::Right((None, _)) => break,
                Either::Left((Some((input, completion_tx)), _)) => {
                    let mut completion_result = Ok(());
                    let ack_number = create_ack(counter);
                    counter += 1;
                    if let Some(message) = I::input_to_message(&input, ack_number) {
                        mcu_tx.send(message.clone()).await?;
                        let time_start = std::time::Instant::now();
                        'ack_number_match: loop {
                            // decrease timeout each iteration
                            let time_until_timeout = TIMEOUT - time_start.elapsed();
                            match timeout(time_until_timeout, ack_rx.next()).await {
                                Ok(Some(ack)) => {
                                    if ack_number != ack.ack_number {
                                        // let's detect weird acks:
                                        // - ack_number for this process
                                        // - with higher counter than the one expected
                                        // (can happen when counter wraps around but should be rare)
                                        if is_ack_for_us(ack.ack_number)
                                            && ack_number < ack.ack_number
                                        {
                                            tracing::warn!(
                                                "Acknowledge number mismatch: Jetson {} <> MCU \
                                                 {}.\nMessage: {}\nDiscarding acknowledge..",
                                                ack_number,
                                                ack.ack_number,
                                                orb_messages::mcu_main::ack::ErrorCode::try_from(
                                                    ack.error
                                                )
                                                .map_or_else(
                                                    |_| ack.error.to_string(),
                                                    |error| error.to_string()
                                                )
                                            );
                                        }
                                        continue 'ack_number_match;
                                    } else if ack.error
                                        == orb_messages::mcu_main::ack::ErrorCode::Success as i32
                                    {
                                        #[allow(let_underscore_drop)]
                                        let _ =
                                            output_tx.send(I::success_ack_output_from_input(input));
                                    // TODO: return Error on MCU Errors and add better Error handling for the callers (f.e. on arguments out of range)
                                    } else if let Ok(error) =
                                        orb_messages::mcu_main::ack::ErrorCode::try_from(ack.error)
                                    {
                                        tracing::error!(
                                            "MCU error: {error}, original message: {message:#?}"
                                        );
                                        // completion_result = Err(Error::msg(format!("µC Error: {}", error)));
                                    } else {
                                        tracing::error!(
                                            "Unknown MCU error code: {}. Perhaps orb-core and MCU \
                                             firmware versions are not compatible",
                                            ack.error
                                        );
                                        // completion_result = Err(Error::msg("Unknown µC Error"));
                                    }
                                }
                                Ok(None) => {
                                    bail!("ack_rx ended unexpectedly");
                                }
                                Err(_) => {
                                    tracing::error!(
                                        "Timed out waiting response from µC with acknowledge \
                                         number: {}",
                                        ack_number
                                    );
                                    completion_result = Err(Error::msg("µC Timeout"));
                                }
                            }
                            // Default is to not match next incoming ack_number
                            break 'ack_number_match;
                        }
                    }
                    if let Some(completion_tx) = completion_tx {
                        completion_tx.send(completion_result).ok();
                    }
                }
                Either::Right((Some(_), _)) => {}
            }
        }
        Ok(())
    }

    async fn handle_output(
        mut mcu_rx: tokio::sync::mpsc::Receiver<orb_messages::mcu_main::mcu_to_jetson::Payload>,
        output_tx: broadcast::Sender<I::Output>,
        mut ack_tx: mpsc::Sender<orb_messages::mcu_main::Ack>,
    ) -> Result<()> {
        let mut nmea_parser = NmeaParser::new();
        let mut nmea_prev_part = None;
        while let Some(output) = mcu_rx.recv().await {
            match output {
                orb_messages::mcu_main::mcu_to_jetson::Payload::Ack(ack) => {
                    ack_tx.send(ack).await?;
                }
                message => {
                    if let Some(output) =
                        I::output_from_message(message, &mut nmea_parser, &mut nmea_prev_part)
                    {
                        #[allow(let_underscore_drop)]
                        let _ = output_tx.send(output);
                    }
                }
            }
        }
        Ok(())
    }

    fn async_tx(
        socket: fd::Tx,
    ) -> tokio::sync::mpsc::Sender<orb_messages::mcu_main::mcu_message::Message> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(ASYNC_TX_CAPACITY);
        spawn_named_thread("mcu-tx", move || {
            while let Some(message) = rx.blocking_recv() {
                let message = orb_messages::mcu_main::McuMessage {
                    version: I::PROTOCOL_VERSION,
                    message: Some(message),
                };
                let bytes = message.encode_length_delimited_to_vec();
                socket
                    .send(I::CAN_ADDRESS, &bytes)
                    .expect("failed to write bytes to the CAN FD socket");
            }
        });
        tx
    }

    fn async_rx(
        socket: fd::Rx,
    ) -> tokio::sync::mpsc::Receiver<orb_messages::mcu_main::mcu_to_jetson::Payload> {
        let (tx, rx) = tokio::sync::mpsc::channel(ASYNC_RX_CAPACITY);
        spawn_named_thread("mcu-rx", move || {
            loop {
                match socket.recv() {
                    Ok(frame) => {
                        if frame.can_id != CAN_FD_ADDR_JETSON {
                            continue;
                        }
                        let data = &frame.data[0..usize::from(frame.len)];
                        match orb_messages::mcu_main::McuMessage::decode_length_delimited(data) {
                            Ok(orb_messages::mcu_main::McuMessage { version, .. })
                                if version != I::PROTOCOL_VERSION =>
                            {
                                tracing::warn!("Received protobuf of unsupported version");
                            }
                            Ok(orb_messages::mcu_main::McuMessage {
                                version: _,
                                message:
                                    Some(orb_messages::mcu_main::mcu_message::Message::MMessage(
                                        orb_messages::mcu_main::McuToJetson {
                                            payload: Some(payload),
                                        },
                                    )),
                            }) => match tx.blocking_send(payload) {
                                Ok(()) => {}
                                Err(_) => break,
                            },
                            Ok(other) => {
                                tracing::warn!("Received unhandled payload from MCU: {:#?}", other);
                            }
                            Err(err) => {
                                tracing::error!("Failed to decode protobuf from MCU: {err}");
                            }
                        }
                    }
                    Err(fd::RecvError::Incomplete(_, _)) => {
                        // That could be an ISO-TP frame.
                        continue;
                    }
                    Err(err) => {
                        tracing::error!("Error receiving from CAN socket: {err:?}");
                    }
                }
            }
        });
        rx
    }
}
