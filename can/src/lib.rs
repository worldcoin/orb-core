//! CAN interface.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

pub mod fd;
pub mod isotp;

use libc::{c_int, c_void, msghdr, size_t, sockaddr, socklen_t, ssize_t};
use std::io;

unsafe fn socket(domain: c_int, ty: c_int, protocol: c_int) -> io::Result<c_int> {
    let fd = unsafe { libc::socket(domain, ty, protocol) };
    if fd == -1 { Err(io::Error::last_os_error()) } else { Ok(fd) }
}

unsafe fn close(fd: c_int) -> io::Result<()> {
    let result = unsafe { libc::close(fd) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

unsafe fn setsockopt(
    socket: c_int,
    level: c_int,
    name: c_int,
    value: *const c_void,
    option_len: socklen_t,
) -> io::Result<()> {
    let result = unsafe { libc::setsockopt(socket, level, name, value, option_len) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

unsafe fn bind(socket: c_int, address: *const sockaddr, address_len: socklen_t) -> io::Result<()> {
    let result = unsafe { libc::bind(socket, address, address_len) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

unsafe fn recv(socket: c_int, buf: *mut c_void, len: size_t, flags: c_int) -> io::Result<ssize_t> {
    let result = unsafe { libc::recv(socket, buf, len, flags) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

unsafe fn send(
    socket: c_int,
    buf: *const c_void,
    len: size_t,
    flags: c_int,
) -> io::Result<ssize_t> {
    let result = unsafe { libc::send(socket, buf, len, flags) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

unsafe fn recvmsg(fd: c_int, msg: *mut msghdr, flags: c_int) -> io::Result<ssize_t> {
    let result = unsafe { libc::recvmsg(fd, msg, flags) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

unsafe fn sendmsg(fd: c_int, msg: *const msghdr, flags: c_int) -> io::Result<ssize_t> {
    let result = unsafe { libc::sendmsg(fd, msg, flags) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}
