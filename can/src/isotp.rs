//! CAN ISO-TP interface.

use super::{bind, close, recv, send, setsockopt, socket};
use libc::{
    c_int, canid_t, sockaddr_can, AF_CAN, CAN_ISOTP, CAN_MAX_DLEN, CAN_MTU, PF_CAN, SOCK_DGRAM,
};
use nix::{net::if_::if_nametoindex, NixPath};
use std::{convert::TryInto, io, mem, ptr};

/// Undocumented can.h constant.
pub const SOL_CAN_BASE: c_int = 100;

/// Undocumented isotp.h constant.
pub const SOL_CAN_ISOTP: c_int = SOL_CAN_BASE + CAN_ISOTP;

/// Pass struct `CanIsoTpOptions`.
pub const CAN_ISOTP_OPTS: c_int = 1;

/// Pass struct `CanIsotpFcOptions`.
pub const CAN_ISOTP_RECV_FC: c_int = 2;

/// Pass struct `CanIsotpFcOptions`.
pub const CAN_ISOTP_LL_OPTS: c_int = 5;

const CAN_ISOTP_OPTIONS_SIZE: usize = mem::size_of::<CanIsotpOptions>();

const CAN_ISOTP_FC_OPTIONS_SIZE: usize = mem::size_of::<CanIsotpFcOptions>();

const CAN_ISOTP_LL_OPTIONS_SIZE: usize = mem::size_of::<CanIsotpLlOptions>();

/// ISO-TP options.
#[repr(C)]
pub struct CanIsotpOptions {
    /// Set flags for isotp behaviour.
    flags: u32,
    /// Frame transmission time (`N_As/N_Ar`) time in nano secs.
    frame_txtime: u32,
    /// Set address for extended addressing.
    ext_address: u8,
    /// Set content of padding byte (tx).
    txpad_content: u8,
    /// Set content of padding byte (rx).
    rxpad_content: u8,
    /// Set address for extended addressing.
    rx_ext_address: u8,
}

/// Flow control options.
#[repr(C)]
pub struct CanIsotpFcOptions {
    /// Blocksize provided in FC frame.
    bs: u8,
    /// Separation time provided in FC frame.
    stmin: u8,
    /// Max. number of wait frame transmiss.
    wftmax: u8,
}

/// Link layer options.
#[repr(C)]
pub struct CanIsotpLlOptions {
    /// Generated & accepted CAN frame type.
    mtu: u8,
    /// Tx link layer data length in bytes (configured maximum payload length).
    tx_dl: u8,
    /// Set into struct `canfd_frame.flags` at frame creation.
    tx_flags: u8,
}

/// CAN ISO-TP socket.
#[derive(Clone)]
pub struct Socket {
    socket: c_int,
}

impl Socket {
    /// Creates a new CAN ISO-TP socket.
    pub fn new(bs: u8) -> io::Result<Self> {
        let socket = unsafe { socket(PF_CAN, SOCK_DGRAM, CAN_ISOTP)? };
        let can_isotp_options = CanIsotpOptions {
            flags: 0x00,
            frame_txtime: 0x00,
            ext_address: 0x00,
            txpad_content: 0xCC,
            rxpad_content: 0xCC,
            rx_ext_address: 0x00,
        };
        let can_isotp_fc_options = CanIsotpFcOptions { bs, stmin: 0x00, wftmax: 0 };
        let can_isotp_ll_options = CanIsotpLlOptions {
            mtu: CAN_MTU.try_into().unwrap(),
            tx_dl: CAN_MAX_DLEN.try_into().unwrap(),
            tx_flags: 0x00,
        };
        unsafe {
            setsockopt(
                socket,
                SOL_CAN_ISOTP,
                CAN_ISOTP_OPTS,
                ptr::addr_of!(can_isotp_options).cast(),
                CAN_ISOTP_OPTIONS_SIZE.try_into().unwrap(),
            )?;
            setsockopt(
                socket,
                SOL_CAN_ISOTP,
                CAN_ISOTP_RECV_FC,
                ptr::addr_of!(can_isotp_fc_options).cast(),
                CAN_ISOTP_FC_OPTIONS_SIZE.try_into().unwrap(),
            )?;
            setsockopt(
                socket,
                SOL_CAN_ISOTP,
                CAN_ISOTP_LL_OPTS,
                ptr::addr_of!(can_isotp_ll_options).cast(),
                CAN_ISOTP_LL_OPTIONS_SIZE.try_into().unwrap(),
            )?;
        }
        Ok(Self { socket })
    }

    /// Binds the socket to the given address.
    pub fn bind<T: ?Sized + NixPath>(
        &mut self,
        name: &T,
        tx_id: canid_t,
        rx_id: canid_t,
    ) -> io::Result<()> {
        let mut addr: sockaddr_can = unsafe { mem::zeroed() };
        addr.can_family = AF_CAN.try_into().unwrap();
        addr.can_ifindex = if_nametoindex(name)?.try_into().unwrap();
        addr.can_addr.tp.rx_id = rx_id;
        addr.can_addr.tp.tx_id = tx_id;
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

impl io::Read for Socket {
    #[allow(clippy::cast_sign_loss)]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = unsafe { recv(self.socket, buf.as_mut_ptr().cast(), buf.len(), 0)? };
        Ok(read as _)
    }
}

impl io::Write for Socket {
    #[allow(clippy::cast_sign_loss)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = unsafe { send(self.socket, buf.as_ptr().cast(), buf.len(), 0)? };
        Ok(written as _)
    }

    fn flush(&mut self) -> io::Result<()> {
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
