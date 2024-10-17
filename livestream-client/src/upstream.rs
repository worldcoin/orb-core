//! Upstream for the events stream.

use eyre::Result;
use std::{
    io::prelude::*,
    net::{IpAddr, TcpStream},
};

const PORT: u16 = 9201;

/// Upstream sender.
pub struct Upstream {
    stream: TcpStream,
}

impl Upstream {
    /// Creates a new [`Upstream`].
    pub fn new(ip: IpAddr) -> Result<Self> {
        let stream = TcpStream::connect((ip, PORT))?;
        Ok(Self { stream })
    }

    /// Sends the given input to the upstream.
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let len: u32 = bytes.len().try_into().unwrap();
        self.stream.write_all(&len.to_be_bytes())?;
        self.stream.write_all(bytes)?;
        Ok(())
    }
}
