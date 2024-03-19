//! `mpsc` channel extensions.

use eyre::{bail, Result};
use futures::channel::mpsc::{Receiver, Sender};

/// An extension trait for [`futures::channel::mpsc::Sender`].
pub trait SenderExt<T> {
    /// Sends a message if the queue is not full.
    fn send_now(&mut self, message: T) -> Result<()>;
}

/// An extension trait for [`futures::channel::mpsc::Receiver`].
pub trait ReceiverExt<T> {
    /// Receivies a message if the queue is not empty.
    fn try_recv(&mut self) -> Result<Option<T>>;
}

impl<T> SenderExt<T> for Sender<T> {
    fn send_now(&mut self, message: T) -> Result<()> {
        match self.try_send(message) {
            Ok(()) => Ok(()),
            Err(err) if err.is_full() => Ok(()),
            Err(err) => bail!("message pass failed: {}", err),
        }
    }
}

impl<T> ReceiverExt<T> for Receiver<T> {
    fn try_recv(&mut self) -> Result<Option<T>> {
        match self.try_next() {
            Ok(Some(message)) => Ok(Some(message)),
            Ok(None) => bail!("channel closed unexpectedly"),
            Err(_) => Ok(None),
        }
    }
}
