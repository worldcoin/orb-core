use eyre::{eyre, Result};
use futures::prelude::*;
use std::{
    mem::take,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    net::{TcpListener, TcpStream},
};

const PORT: u16 = 9201;

pub struct Upstream {
    listener: TcpListener,
    stream: Option<(TcpStream, EventReader)>,
}

pub enum Event {
    Connected(SocketAddr),
    Closed,
    UiEvents(Vec<livestream_event::Event>),
}

#[derive(Default)]
struct EventReader {
    len: [u8; 4],
    len_read: usize,
    buf: Vec<u8>,
    buf_read: usize,
}

impl Upstream {
    pub async fn new() -> Result<Self> {
        let listener = TcpListener::bind(format!("0.0.0.0:{PORT}")).await?;
        Ok(Self { listener, stream: None })
    }
}

impl Stream for Upstream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some((stream, event_reader)) = &mut self.stream {
            match event_reader.poll_stream(cx, Pin::new(stream)) {
                Poll::Ready(events) => {
                    *event_reader = EventReader::default();
                    match events {
                        Ok(Some(events)) => {
                            return Poll::Ready(Some(Ok(Event::UiEvents(events))));
                        }
                        Ok(None) => {
                            self.stream = None;
                            return Poll::Ready(Some(Ok(Event::Closed)));
                        }
                        Err(err) => return Poll::Ready(Some(Err(err))),
                    }
                }
                Poll::Pending => {}
            }
        }
        match self.listener.poll_accept(cx) {
            Poll::Ready(Ok((stream, addr))) => {
                self.stream = Some((stream, EventReader::default()));
                Poll::Ready(Some(Ok(Event::Connected(addr))))
            }
            Poll::Ready(Err(err)) => {
                Poll::Ready(Some(Err(eyre!("Error accepting connection: {err}"))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl EventReader {
    fn poll_stream(
        &mut self,
        cx: &mut Context<'_>,
        mut stream: Pin<&mut TcpStream>,
    ) -> Poll<Result<Option<Vec<livestream_event::Event>>>> {
        loop {
            if self.len_read < 4 {
                let mut read_buf = ReadBuf::new(&mut self.len[self.len_read..]);
                match stream.as_mut().poll_read(cx, &mut read_buf) {
                    Poll::Ready(Ok(())) if read_buf.filled().is_empty() => {
                        return Poll::Ready(Ok(None));
                    }
                    Poll::Ready(Ok(())) => self.len_read += read_buf.filled().len(),
                    Poll::Ready(Err(err)) => {
                        return Poll::Ready(Err(eyre!("Error reading from stream: {err}")));
                    }
                    Poll::Pending => return Poll::Pending,
                }
                if self.len_read == 4 {
                    self.buf = vec![0; u32::from_be_bytes(self.len) as usize];
                }
            } else if self.buf_read < u32::from_be_bytes(self.len) as usize {
                let mut read_buf = ReadBuf::new(&mut self.buf[self.buf_read..]);
                match stream.as_mut().poll_read(cx, &mut read_buf) {
                    Poll::Ready(Ok(())) if read_buf.filled().is_empty() => {
                        return Poll::Ready(Ok(None));
                    }
                    Poll::Ready(Ok(())) => self.buf_read += read_buf.filled().len(),
                    Poll::Ready(Err(err)) => {
                        return Poll::Ready(Err(eyre!("Error reading from stream: {err}")));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            } else {
                break;
            }
        }
        let bytes = take(&mut self.buf);
        match rkyv::from_bytes(&bytes) {
            Ok(input) => Poll::Ready(Ok(Some(input))),
            Err(err) => Poll::Ready(Err(eyre!("Failed to deserialize input: {err}"))),
        }
    }
}
