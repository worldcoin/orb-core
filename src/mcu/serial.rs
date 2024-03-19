//! Serial MCU interface.

use super::Interface;
use crate::utils::spawn_named_thread;
use eyre::Result;
use futures::{channel::mpsc, prelude::*};
use orb_messages;
use orb_uart::Device;
use prost::Message;
use std::{io::Write, marker::PhantomData};
use tokio::runtime;

const SERIAL_DEVICE: &str = "/dev/ttyTHS0";

/// Serial interface.
pub struct Serial<I: Interface>(PhantomData<I>);

impl<I: Interface> Serial<I> {
    /// Spawns a new serial interface.
    #[allow(clippy::missing_panics_doc)]
    pub fn spawn(mut input_rx: mpsc::Receiver<I::Input>) -> Result<()> {
        let mut device = Device::open(SERIAL_DEVICE)?;
        spawn_named_thread("mcu-uart-tx", move || {
            let rt = runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create a new tokio runtime");
            let mut ack_number = 0;
            while let Some(message) = rt.block_on(input_rx.next()) {
                Self::write_message(&mut device, &message, ack_number)
                    .expect("failed to transmit a message to MCU via UART");
                ack_number += 1;
            }
        });
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn write_message(w: &mut impl Write, input: &I::Input, ack_number: u32) -> Result<()> {
        if let Some(message) = I::input_to_message(input, ack_number) {
            let message = orb_messages::mcu_main::McuMessage {
                version: I::PROTOCOL_VERSION,
                message: Some(message),
            };
            // UART message: magic (2B) + size (2B) + payload (protobuf-encoded McuMessage)
            let mut bytes = vec![0x8E, 0xAD];
            let mut payload = message.encode_length_delimited_to_vec();
            let mut size = Vec::from((payload.len() as u16).to_le_bytes());
            bytes.append(&mut size);
            bytes.append(&mut payload);
            w.write_all(&bytes)?;
        }
        Ok(())
    }
}
