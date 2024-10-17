//! Microcontroller interface.

#![allow(clippy::non_ascii_literal)]

pub mod can;
pub mod main;

use std::pin::Pin;

pub use self::main::Main;

use crate::ext::mpsc::SenderExt as _;
use eyre::{Error, Result};
use futures::{
    channel::{mpsc, oneshot},
    prelude::*,
    stream::Fuse,
};
use nmea_parser::NmeaParser;
use tokio_stream::wrappers::BroadcastStream;

const SEND_RETRY_COUNT: u8 = 3;

type ResultSender = oneshot::Sender<Result<(), Error>>;

/// General microcontroller interface.
pub trait Interface {
    /// Input message.
    type Input: Clone + Send + Sync + 'static;

    /// Output message.
    type Output: Clone + Send + 'static;

    /// Configuration history.
    type Log: Default;

    /// CAN-bus address of the microcontroller.
    const CAN_ADDRESS: u32;

    /// CAN protocol version.
    const PROTOCOL_VERSION: i32;

    /// Saves the input message to the log.
    fn log_input(log: &mut Self::Log, input: &Self::Input);

    /// Converts an input message to a CAN protocol message.
    fn input_to_message(
        input: &Self::Input,
        ack_number: u32,
    ) -> Option<orb_messages::mcu_main::mcu_message::Message>;

    /// Converts a CAN protocol message into an output message.
    fn output_from_message(
        message: orb_messages::mcu_main::mcu_to_jetson::Payload,
        nmea_parser: &mut NmeaParser,
        nmea_prev_part: &mut Option<(u32, String)>,
    ) -> Option<Self::Output>;

    /// Converts an input message into an SuccessAck output message.
    fn success_ack_output_from_input(input: Self::Input) -> Self::Output;
}

/// General microcontroller trait.
///
/// It is split into two traits `Mcu` and `Interface` because of Rust object
/// safety rules.
pub trait Mcu<I: Interface>: Send {
    /// Returns a new handler to the shared microcontroller interface.
    fn clone(&self) -> Box<dyn Mcu<I>>;

    /// Returns a reference to the input message sender.
    fn tx(&self) -> &mpsc::Sender<(I::Input, Option<ResultSender>)>;

    /// Returns a mutable reference to the input message sender.
    fn tx_mut(&mut self) -> &mut mpsc::Sender<(I::Input, Option<ResultSender>)>;

    /// Returns a reference to the stream of output messages.
    fn rx(&self) -> &Fuse<BroadcastStream<I::Output>>;

    /// Returns a mutable reference to the stream of output messages.
    fn rx_mut(&mut self) -> &mut Fuse<BroadcastStream<I::Output>>;

    /// Returns a mutable reference to the configuration history.
    fn log_mut(&mut self) -> &mut Option<I::Log>;

    /// Sends a message to the microcontroller and waits for the acknowledge.
    fn send(&mut self, input: I::Input) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut retries = SEND_RETRY_COUNT;
            'retry: loop {
                let (completion_tx, completion_rx) = oneshot::channel();
                self.tx_mut().send((input.clone(), Some(completion_tx))).await?;
                if let Err(error) = completion_rx.await? {
                    if retries > 0 {
                        tracing::warn!("Retrying last µC message... [{}]", retries);
                        retries -= 1;
                        continue 'retry;
                    }
                    tracing::error!("Maximum µC send retries reached, aborting with Error");
                    return Err(error);
                };
                break 'retry;
            }
            if let Some(log) = self.log_mut() {
                I::log_input(log, &input);
            }
            Ok(())
        })
    }

    /// Attempts to send a message to the microcontroller without waiting for
    /// the acknowledge.
    fn send_now(&mut self, input: I::Input) -> Result<()> {
        if let Some(log) = self.log_mut() {
            I::log_input(log, &input);
        }
        self.tx_mut().send_now((input, None))?;
        Ok(())
    }

    /// Sends a message to the microcontroller over UART interface.
    fn send_uart(&mut self, _input: I::Input) -> Result<()> {
        unimplemented!();
    }

    /// Starts logging configuration history.
    fn log_start(&mut self) {
        *self.log_mut() = Some(Default::default());
    }

    /// Returns the configuration history and stops logging new values.
    fn log_stop(&mut self) -> I::Log {
        self.log_mut().take().unwrap_or_default()
    }
}
