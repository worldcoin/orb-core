//! CAN MCU interface.

use super::{Interface, ResultSender};
use crate::utils::spawn_named_thread;
use eyre::{bail, Error, Result};
use futures::{channel::mpsc, future::try_join, prelude::*};
use libc::CAN_EFF_FLAG;
use nmea_parser::NmeaParser;
use orb_can::fd;
use orb_messages;
use prost::Message;
use std::{marker::PhantomData, time::Duration};
use tokio::{runtime, sync::broadcast, task, time::timeout};

const ASYNC_TX_CAPACITY: usize = 100;
const ASYNC_RX_CAPACITY: usize = 100;
const CAN_SOCKET: &str = "can0";
const CAN_FD_ADDR_JETSON: u32 = 0x80 | CAN_EFF_FLAG;
const TIMEOUT: Duration = Duration::from_millis(300);

/// CAN interface.
pub struct Can<I: Interface>(PhantomData<I>);

impl<I: Interface> Can<I> {
    /// Spawns a new CAN interface.
    pub fn spawn(
        input_rx: mpsc::Receiver<(I::Input, Option<ResultSender>)>,
        output_tx: broadcast::Sender<I::Output>,
    ) -> Result<()> {
        let (tx, rx) = fd::open(CAN_SOCKET)?;
        let tx = Self::async_tx(tx);
        let rx = Self::async_rx(rx);
        let (ack_tx, ack_rx) = mpsc::channel(1);
        task::spawn(async move {
            let input_fut = Self::handle_input(tx, input_rx, ack_rx, output_tx.clone());
            let output_fut = Self::handle_output(rx, output_tx, ack_tx);
            match try_join(input_fut, output_fut).await {
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
        mut mcu_tx: mpsc::Sender<orb_messages::mcu_main::mcu_message::Message>,
        mut input_rx: mpsc::Receiver<(I::Input, Option<ResultSender>)>,
        mut ack_rx: mpsc::Receiver<orb_messages::mcu_main::Ack>,
        output_tx: broadcast::Sender<I::Output>,
    ) -> Result<()> {
        let mut ack_number = 0;
        while let Some((input, completion_tx)) = input_rx.next().await {
            let mut completion_result = Ok(());
            if let Some(message) = I::input_to_message(&input, ack_number) {
                mcu_tx.send(message.clone()).await?;
                'ack_number_match: loop {
                    match timeout(TIMEOUT, ack_rx.next()).await {
                        Ok(Some(ack)) => {
                            if ack_number != ack.ack_number {
                                // This should only happen for lower ack_number than the jetson expects due to timeouts
                                tracing::error!(
                                    "Acknowledge number mismatch: Jetson {} <> MCU {}.\nMessage: \
                                     {}\nDiscarding acknowledge..",
                                    ack_number,
                                    ack.ack_number,
                                    orb_messages::mcu_main::ack::ErrorCode::try_from(ack.error)
                                        .map_or_else(
                                            |_| ack.error.to_string(),
                                            |error| error.to_string()
                                        )
                                );
                                // continue waiting for the correct ack_number since this can happen due to timeout
                                if ack_number > ack.ack_number {
                                    continue 'ack_number_match;
                                }
                            } else if ack.error
                                == orb_messages::mcu_main::ack::ErrorCode::Success as i32
                            {
                                #[allow(let_underscore_drop)]
                                let _ = output_tx.send(I::success_ack_output_from_input(input));
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
                                "Timed out waiting response from µC with acknowledge number: {}",
                                ack_number
                            );
                            completion_result = Err(Error::msg("µC Timeout"));
                        }
                    }
                    // Default is to not match next incoming ack_number
                    break 'ack_number_match;
                }
                ack_number += 1;
            }
            if let Some(completion_tx) = completion_tx {
                completion_tx.send(completion_result).ok();
            }
        }
        Ok(())
    }

    async fn handle_output(
        mut mcu_rx: mpsc::Receiver<orb_messages::mcu_main::mcu_to_jetson::Payload>,
        output_tx: broadcast::Sender<I::Output>,
        mut ack_tx: mpsc::Sender<orb_messages::mcu_main::Ack>,
    ) -> Result<()> {
        let mut nmea_parser = NmeaParser::new();
        let mut nmea_prev_part = None;
        while let Some(output) = mcu_rx.next().await {
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

    fn async_tx(socket: fd::Tx) -> mpsc::Sender<orb_messages::mcu_main::mcu_message::Message> {
        let (tx, mut rx) = mpsc::channel(ASYNC_TX_CAPACITY);
        spawn_named_thread("mcu-tx", move || {
            let rt = runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create a new tokio runtime");
            while let Some(message) = rt.block_on(rx.next()) {
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

    fn async_rx(socket: fd::Rx) -> mpsc::Receiver<orb_messages::mcu_main::mcu_to_jetson::Payload> {
        let (mut tx, rx) = mpsc::channel(ASYNC_RX_CAPACITY);
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
                            }) => match tx.try_send(payload) {
                                Ok(()) => {}
                                Err(err) if err.is_disconnected() => break,
                                Err(err) => tracing::error!("µC RX queue error: {err}"),
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
