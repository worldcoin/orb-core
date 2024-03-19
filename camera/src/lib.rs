//! High-level wrapper for video4linux capturing interface.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod buffer;
mod device;
mod wait;

pub use self::{
    buffer::{Buffer, Dequeued},
    device::{Device, Format},
    wait::Waiter,
};

use libc::{
    c_char, c_int, c_uint, c_ulong, c_void, fd_set, off_t, size_t, ssize_t, timeval, MAP_FAILED,
};
use std::{io, time::Duration};

/// Returns the timestamp of the moment of now of the same format as the frame
/// timestamp.
pub fn now() -> nix::Result<Duration> {
    nix::time::clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC).map(Duration::from)
}

unsafe fn open(path: *const c_char, oflag: c_int) -> io::Result<c_int> {
    let fd = unsafe { libc::open(path, oflag) };
    if fd == -1 { Err(io::Error::last_os_error()) } else { Ok(fd) }
}

unsafe fn close(fd: c_int) -> io::Result<()> {
    let result = unsafe { libc::close(fd) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

unsafe fn ioctl(fd: c_int, request: c_ulong, argp: *mut c_void) -> io::Result<Option<c_int>> {
    let result = unsafe { libc::ioctl(fd, request, argp) };
    if result == -1 {
        let err = io::Error::last_os_error();
        match err.kind() {
            io::ErrorKind::WouldBlock => Ok(None),
            _ => Err(err),
        }
    } else {
        Ok(Some(result))
    }
}

unsafe fn mmap(
    addr: *mut c_void,
    len: size_t,
    prot: c_int,
    flags: c_int,
    fd: c_int,
    offset: off_t,
) -> io::Result<*mut c_void> {
    let ptr = unsafe { libc::mmap(addr, len, prot, flags, fd, offset) };
    if ptr == MAP_FAILED { Err(io::Error::last_os_error()) } else { Ok(ptr) }
}

unsafe fn munmap(addr: *mut c_void, len: size_t) -> io::Result<()> {
    let result = unsafe { libc::munmap(addr, len) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

unsafe fn select(
    nfds: c_int,
    readfs: *mut fd_set,
    writefds: *mut fd_set,
    errorfds: *mut fd_set,
    timeout: *mut timeval,
) -> io::Result<c_int> {
    let result = unsafe { libc::select(nfds, readfs, writefds, errorfds, timeout) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

unsafe fn eventfd(init: c_uint, flags: c_int) -> io::Result<c_int> {
    let fd = unsafe { libc::eventfd(init, flags) };
    if fd == -1 { Err(io::Error::last_os_error()) } else { Ok(fd) }
}

unsafe fn read(fd: c_int, buf: *mut c_void, count: size_t) -> io::Result<ssize_t> {
    let result = unsafe { libc::read(fd, buf, count) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

unsafe fn write(fd: c_int, buf: *const c_void, count: size_t) -> io::Result<ssize_t> {
    let result = unsafe { libc::write(fd, buf, count) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}
