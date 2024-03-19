use super::{close, ioctl, open, read, tcdrain, tcflush, tcgetattr, tcsetattr, write};
use libc::{
    c_int, speed_t, termios, B1000000, CBAUD, CLOCAL, CMSPAR, CREAD, CRTSCTS, CS8, CSIZE, CSTOPB,
    ECHO, ECHOCTL, ECHOE, ECHOK, ECHOKE, ECHONL, ICANON, ICRNL, IEXTEN, IGNBRK, IGNCR, INLCR,
    INPCK, ISIG, ISTRIP, IXANY, IXOFF, IXON, OCRNL, ONLCR, OPOST, O_CLOEXEC, O_EXCL, O_NOCTTY,
    O_RDWR, PARENB, PARMRK, PARODD, TCIFLUSH, TCSANOW, TIOCMBIS, TIOCM_DTR, TIOCM_RTS, VMIN, VTIME,
};
use std::{ffi::CString, io, mem, os::unix::ffi::OsStrExt, path::Path, ptr};

const BAUD_RATE: speed_t = B1000000;

/// Bi-directional UART handle.
#[derive(Clone)]
pub struct Device {
    fd: c_int,
}

impl Device {
    /// Opens a serial interface.
    ///
    /// # Panics
    ///
    /// If failed to open the device.
    pub fn open<T: AsRef<Path>>(path: T) -> io::Result<Self> {
        let path = CString::new(path.as_ref().as_os_str().as_bytes())?;
        let fd = unsafe { open(path.as_ptr(), O_RDWR | O_NOCTTY | O_EXCL | O_CLOEXEC).unwrap() };

        let mut termios: termios = unsafe { mem::zeroed() };
        unsafe { tcgetattr(fd, &mut termios)? };
        termios.c_cflag &= !(PARENB | PARODD | CMSPAR | CSIZE | CRTSCTS | CBAUD);
        termios.c_cflag |= CLOCAL | CREAD | CS8 | CSTOPB | BAUD_RATE;
        termios.c_lflag &=
            !(ICANON | ECHO | ECHOE | ECHOK | ECHONL | ISIG | IEXTEN | ECHOCTL | ECHOKE);
        termios.c_oflag &= !(OPOST | ONLCR | OCRNL);
        termios.c_iflag &=
            !(INLCR | IGNCR | ICRNL | IGNBRK | INPCK | ISTRIP | IXON | IXOFF | IXANY | PARMRK);
        termios.c_ispeed = BAUD_RATE;
        termios.c_ospeed = BAUD_RATE;
        termios.c_cc[VMIN] = 1;
        termios.c_cc[VTIME] = 0;
        unsafe { tcsetattr(fd, TCSANOW, &termios)? };

        let mut bits: c_int = TIOCM_DTR | TIOCM_RTS;
        unsafe { ioctl(fd, TIOCMBIS, ptr::addr_of_mut!(bits).cast())? };
        unsafe { tcflush(fd, TCIFLUSH)? };

        Ok(Self { fd })
    }
}

impl io::Read for Device {
    #[allow(clippy::cast_sign_loss)]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = unsafe { read(self.fd, buf.as_mut_ptr().cast(), buf.len())? };
        Ok(read as _)
    }
}

impl io::Write for Device {
    #[allow(clippy::cast_sign_loss)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = unsafe { write(self.fd, buf.as_ptr().cast(), buf.len())? };
        Ok(written as _)
    }

    fn flush(&mut self) -> io::Result<()> {
        unsafe { tcdrain(self.fd) }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            if let Err(err) = close(self.fd) {
                log::error!("Couldn't close serial interface descriptor: {}", err);
            }
        }
    }
}
