//! `broadcast` channel extensions.

#![allow(clippy::module_name_repetitions)]

use eyre::{bail, eyre, Result};
use futures::{future::FusedFuture, prelude::*, ready, stream::FusedStream};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// An extension trait for [`tokio::sync::broadcast::Receiver`].
pub trait ReceiverExt<T: Clone + Send + 'static>:
    Stream<Item = Result<T, BroadcastStreamRecvError>> + Unpin
{
    /// Creates a future that resolves to the next item in the stream. Returns
    /// `Err` if the stream ended.
    fn next_broadcast(&mut self) -> NextBroadcast<'_, Self> {
        NextBroadcast { stream: self }
    }

    /// Receivies a message if the queue is not empty.
    fn try_recv_broadcast(&mut self) -> Result<Option<T>> {
        self.next_broadcast().now_or_never().transpose()
    }

    /// Clear all pending messages.
    fn clear(&mut self) -> Result<()> {
        while let Some(output) = self.next().now_or_never() {
            match output {
                Some(Ok(_) | Err(BroadcastStreamRecvError::Lagged(_))) => {}
                None => bail!("channel closed unexpectedly"),
            }
        }
        Ok(())
    }
}

impl<T, S> ReceiverExt<T> for S
where
    T: Clone + Send + 'static,
    S: Stream<Item = Result<T, BroadcastStreamRecvError>> + Unpin + ?Sized,
{
}

/// Future for the [`ReceiverExt::next_broadcast`] method.
pub struct NextBroadcast<'a, S: Stream + ?Sized> {
    stream: &'a mut S,
}

impl<'a, T, S: Stream<Item = Result<T, BroadcastStreamRecvError>> + Unpin + ?Sized> Future
    for NextBroadcast<'a, S>
{
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            break match ready!(self.stream.poll_next_unpin(cx)) {
                Some(Ok(output)) => Poll::Ready(Ok(output)),
                Some(Err(BroadcastStreamRecvError::Lagged(count))) => {
                    tracing::warn!("receiver lagged behind by {} items", count);
                    continue;
                }
                None => Poll::Ready(Err(eyre!("broadcast channel closed unexpectedly"))),
            };
        }
    }
}

impl<'a, T, S: Stream<Item = Result<T, BroadcastStreamRecvError>> + FusedStream + Unpin + ?Sized>
    FusedFuture for NextBroadcast<'a, S>
{
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}
