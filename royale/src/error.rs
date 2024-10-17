#![cfg_attr(
    not(all(target_arch = "aarch64", target_os = "linux")),
    allow(unused_imports, unused_variables)
)]

use std::{ffi::CStr, fmt, os::raw::c_int};

/// Royale SDK error.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub struct Error(c_int);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        {
            let camera_status_string = unsafe { royale_sys::camera_status_to_string(self.0) };
            let c_str = unsafe { CStr::from_ptr(camera_status_string) };
            let result = f.write_str(&c_str.to_string_lossy());
            unsafe { royale_sys::delete_string(camera_status_string) };
            result
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
        Ok(())
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub(crate) fn result_from(status: c_int) -> Result<(), Error> {
    if unsafe { royale_sys::is_camera_status_success(status) } {
        Ok(())
    } else {
        Err(Error(status))
    }
}
