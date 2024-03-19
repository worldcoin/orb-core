//! Stream extensions.

#![allow(clippy::module_name_repetitions)]

use eyre::{eyre, Result};
use futures::{future::FusedFuture, prelude::*, ready, stream::FusedStream};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// An extension trait for streams.
pub trait StreamExt: Stream {
    /// Creates a future that resolves to the next item in the stream. Returns
    /// `Err` if the stream ended.
    fn next_ok(&mut self) -> NextOk<'_, Self> {
        NextOk { stream: self }
    }

    /// Creates a future that resolves to the next item in the stream. Panics if
    /// the stream ended.
    fn next_unwrap(&mut self) -> NextUnwrap<'_, Self> {
        NextUnwrap { stream: self }
    }
}

impl<T: Stream + ?Sized> StreamExt for T {}

/// Future for the [`StreamExt::next_ok`] method.
pub struct NextOk<'a, S: Stream + ?Sized> {
    stream: &'a mut S,
}

/// Future for the [`StreamExt::next_unwrap`] method.
pub struct NextUnwrap<'a, S: Stream + ?Sized> {
    stream: &'a mut S,
}

impl<'a, S: Stream + Unpin + ?Sized> Future for NextOk<'a, S> {
    type Output = Result<S::Item>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.stream.poll_next_unpin(cx)) {
            Some(output) => Poll::Ready(Ok(output)),
            None => Poll::Ready(Err(eyre!("stream ended unexpectedly"))),
        }
    }
}

impl<'a, S: Stream + Unpin + ?Sized> Future for NextUnwrap<'a, S> {
    type Output = S::Item;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.stream.poll_next_unpin(cx)) {
            Some(output) => Poll::Ready(output),
            None => panic!("stream ended unexpectedly"),
        }
    }
}

impl<'a, S: Stream + FusedStream + Unpin + ?Sized> FusedFuture for NextOk<'a, S> {
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}

impl<'a, S: Stream + FusedStream + Unpin + ?Sized> FusedFuture for NextUnwrap<'a, S> {
    fn is_terminated(&self) -> bool {
        self.stream.is_terminated()
    }
}
