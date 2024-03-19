//! CAN FD interface.

use super::{bind, close, recvmsg, sendmsg, setsockopt, socket};
use libc::{
    c_int, canfd_frame, canid_t, iovec, msghdr, sockaddr_can, AF_CAN, CANFD_MTU, CAN_RAW,
    CAN_RAW_FD_FRAMES, PF_CAN, SOCK_CLOEXEC, SOCK_RAW, SOL_CAN_RAW,
};
use nix::{net::if_::if_nametoindex, NixPath};
use std::{convert::TryInto, io, mem, ptr, sync::Arc};
use thiserror::Error;

/// Error returned by [`Tx::send`].
#[derive(Error, Debug)]
pub enum SendError {
    /// IO error.
    #[error("IO error: {}", .0)]
    Io(io::Error),
    /// Incomplete write.
    #[error("Incomplete write: {}/{} bytes written", .0, .1)]
    Incomplete(isize, usize),
    /// Data size is too large
    #[error("Data size is too large: {} bytes of max {}", .0, .1)]
    SizeTooLarge(usize, usize),
}

/// Error returned by [`Rx::recv`].
#[derive(Error, Debug)]
pub enum RecvError {
    /// IO error.
    #[error("IO error: {}", .0)]
    Io(io::Error),
    /// Incomplete read.
    #[error("Incomplete read: {}/{} bytes read", .0, .1)]
    Incomplete(isize, usize),
}

/// CAN FD socket transmitter.
#[derive(Debug)]
pub struct Tx {
    inner: Arc<Socket>,
}

/// CAN FD socket receiver.
#[derive(Debug)]
pub struct Rx {
    inner: Arc<Socket>,
}

#[derive(Debug)]
struct Socket {
    socket: c_int,
}

/// Creates a new CAN FD socket, returning its tx/rx pair.
pub fn open<T: ?Sized + NixPath>(name: &T) -> io::Result<(Tx, Rx)> {
    let mut socket = Socket::new()?;
    socket.bind(name)?;
    let socket = Arc::new(socket);
    let tx = Tx::from(Arc::clone(&socket));
    let rx = Rx::from(socket);
    Ok((tx, rx))
}

impl From<Arc<Socket>> for Tx {
    fn from(inner: Arc<Socket>) -> Self {
        Self { inner }
    }
}

impl From<Arc<Socket>> for Rx {
    fn from(inner: Arc<Socket>) -> Self {
        Self { inner }
    }
}

impl Tx {
    /// Sends `data` with specific `can_id`.
    pub fn send(&self, can_id: canid_t, data: &[u8]) -> Result<(), SendError> {
        let mut frame: canfd_frame = unsafe { mem::zeroed() };
        frame.can_id = can_id;
        frame.flags = 0x0F;
        if data.len() > frame.data.len() {
            return Err(SendError::SizeTooLarge(data.len(), frame.data.len()));
        }
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), frame.data.as_mut_ptr(), data.len()) };
        // ensure discrete CAN FD length values 0..8, 12, 16, 20, 24, 32, 48, 64
        frame.len = can_fd_dlc_to_len(can_fd_len_to_dlc(data.len()));
        let mut iov = iovec { iov_base: ptr::addr_of_mut!(frame).cast(), iov_len: CANFD_MTU };
        let msg = msghdr {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iov),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        let written = unsafe { sendmsg(self.inner.socket, ptr::addr_of!(msg).cast(), 0)? };
        if written != CANFD_MTU.try_into().unwrap() {
            return Err(SendError::Incomplete(written, CANFD_MTU));
        }
        Ok(())
    }
}

impl Rx {
    /// Receives a frame.
    pub fn recv(&self) -> Result<canfd_frame, RecvError> {
        let mut frame: canfd_frame = unsafe { mem::zeroed() };
        let mut iov = iovec { iov_base: ptr::addr_of_mut!(frame).cast(), iov_len: CANFD_MTU };
        let mut msg = msghdr {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: ptr::addr_of_mut!(iov),
            msg_iovlen: 1,
            msg_control: ptr::null_mut(),
            msg_controllen: 0,
            msg_flags: 0,
        };
        let read = unsafe { recvmsg(self.inner.socket, ptr::addr_of_mut!(msg).cast(), 0)? };
        if read != CANFD_MTU.try_into().unwrap() {
            return Err(RecvError::Incomplete(read, CANFD_MTU));
        }
        Ok(frame)
    }
}

impl Socket {
    fn new() -> io::Result<Self> {
        // open socket
        let socket = unsafe { socket(PF_CAN, SOCK_RAW | SOCK_CLOEXEC, CAN_RAW)? };
        // try to switch the socket into CAN FD mode
        let enable_canfd: c_int = 1;
        unsafe {
            setsockopt(
                socket,
                SOL_CAN_RAW,
                CAN_RAW_FD_FRAMES,
                ptr::addr_of!(enable_canfd).cast(),
                mem::size_of_val(&enable_canfd).try_into().unwrap(),
            )?;
        }
        Ok(Self { socket })
    }

    fn bind<T: ?Sized + NixPath>(&mut self, name: &T) -> io::Result<()> {
        let mut addr: sockaddr_can = unsafe { mem::zeroed() };
        addr.can_family = AF_CAN.try_into().unwrap();
        addr.can_ifindex = if_nametoindex(name)?.try_into().unwrap();
        unsafe {
            bind(
                self.socket,
                ptr::addr_of!(addr).cast(),
                mem::size_of_val(&addr).try_into().unwrap(),
            )?;
        }
        Ok(())
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        unsafe {
            if let Err(err) = close(self.socket) {
                log::error!("Couldn't close CAN socket: {}", err);
            }
        }
    }
}

impl From<io::Error> for SendError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<io::Error> for RecvError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Maps the sanitized data length to an appropriate data length code.
fn can_fd_len_to_dlc(len: usize) -> u8 {
    const LEN_TO_DLC: &[u8] = &[
        0, 1, 2, 3, 4, 5, 6, 7, 8, /* 0 - 8 */
        9, 9, 9, 9, /* 9 - 12 */
        10, 10, 10, 10, /* 13 - 16 */
        11, 11, 11, 11, /* 17 - 20 */
        12, 12, 12, 12, /* 21 - 24 */
        13, 13, 13, 13, 13, 13, 13, 13, /* 25 - 32 */
        14, 14, 14, 14, 14, 14, 14, 14, /* 33 - 40 */
        14, 14, 14, 14, 14, 14, 14, 14, /* 41 - 48 */
        15, 15, 15, 15, 15, 15, 15, 15, /* 49 - 56 */
        15, 15, 15, 15, 15, 15, 15, 15,
    ];
    if len <= 64 { LEN_TO_DLC[len] } else { 0xF }
}

/// Gets data length from raw data length code (DLC).
fn can_fd_dlc_to_len(dlc: u8) -> u8 {
    const DLC_TO_LEN: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];
    DLC_TO_LEN[usize::from(dlc & 0x0F)]
}
