#![cfg_attr(not(all(target_arch = "aarch64", target_os = "linux")), allow(unused_imports))]

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use crate::error::result_from;
use crate::{error::Error, Frame};
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use std::ffi::{CStr, CString};
use std::{os::raw::c_void, ptr, sync::mpsc};

/// Royale SDK camera interface.
pub struct Camera {
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    camera_ptr: *mut royale_sys::Camera,
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    listener_ptr: *mut royale_sys::DataListener,
    frame_rx: mpsc::Receiver<Frame>,
}

/// Error returned from `Camera::attach`.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// Depth camera not found
    #[error("depth camera not found")]
    NotFound,
    /// Generic error code.
    #[error("{}", 0)]
    Generic(Error),
}

impl Camera {
    /// Attaches to a Royale SDK camera connected via USB. If there are multiple
    /// cameras, this method connects to the first one.
    ///
    /// # Errors
    ///
    /// See [`AttachError`] for all possible errors.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn attach() -> Result<Self, AttachError> {
        type Closure = Box<dyn Fn(*const royale_sys::Frame)>;
        extern "C" fn callback(frame: *const royale_sys::Frame, payload: *mut c_void) {
            unsafe { (*payload.cast::<Closure>())(frame) };
        }
        let (frame_tx, frame_rx) = mpsc::channel();
        let mut camera_ptr = ptr::null_mut();
        let mut listener_ptr = ptr::null_mut();
        let closure: Closure = Box::new(move |frame: *const royale_sys::Frame| {
            let frame = unsafe { Frame::obtain(frame) };
            let _ = frame_tx.send(frame);
        });
        let closure = Box::into_raw(Box::new(closure));
        if let Err(error) = result_from(unsafe {
            royale_sys::camera_attach(
                Some(callback),
                closure.cast(),
                &mut camera_ptr,
                &mut listener_ptr,
            )
        }) {
            if !camera_ptr.is_null() {
                unsafe { royale_sys::camera_delete(camera_ptr, listener_ptr) };
            }
            return Err(AttachError::Generic(error));
        }
        if camera_ptr.is_null() {
            return Err(AttachError::NotFound);
        }
        Ok(Self { camera_ptr, listener_ptr, frame_rx })
    }

    /// Attempts to wait for a frame from this camera.
    #[allow(clippy::missing_panics_doc)]
    #[must_use]
    pub fn recv(&self) -> Frame {
        self.frame_rx.recv().unwrap()
    }

    /// Returns all use cases which are supported by the connected module.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn get_use_cases(&self) -> Result<Vec<String>, Error> {
        let mut output = Vec::new();
        unsafe {
            let use_cases = royale_sys::new_string_vector();
            result_from(royale_sys::camera_get_use_cases(self.camera_ptr, use_cases))?;
            for i in 0..royale_sys::string_vector_length(use_cases) {
                let use_case = royale_sys::string_vector_get(use_cases, i);
                let c_str = CStr::from_ptr(use_case);
                output.push(c_str.to_string_lossy().into_owned());
                royale_sys::delete_string(use_case);
            }
            royale_sys::delete_string_vector(use_cases);
        }
        Ok(output)
    }

    /// Sets the use case for the camera.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    ///
    /// # Panics
    ///
    /// If the use case contains an interior null byte.
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn set_use_case(&self, use_case: &str) -> Result<(), Error> {
        let use_case = CString::new(use_case).unwrap();
        unsafe { result_from(royale_sys::camera_set_use_case(self.camera_ptr, use_case.as_ptr())) }
    }

    /// Gets the maximal frame rate which can be set for the current use case.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn get_max_frame_rate(&self) -> Result<u16, Error> {
        let mut frame_rate = 0;
        unsafe {
            result_from(royale_sys::camera_get_max_frame_rate(self.camera_ptr, &mut frame_rate))?;
        }
        Ok(frame_rate)
    }

    /// Gets the current frame rate which is set for the current use case.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn get_frame_rate(&self) -> Result<u16, Error> {
        let mut frame_rate = 0;
        unsafe {
            result_from(royale_sys::camera_get_frame_rate(self.camera_ptr, &mut frame_rate))?;
        }
        Ok(frame_rate)
    }

    /// Sets the frame rate to a value.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn set_frame_rate(&self, frame_rate: u16) -> Result<(), Error> {
        unsafe { result_from(royale_sys::camera_set_frame_rate(self.camera_ptr, frame_rate)) }
    }

    /// Retrieves the current mode of operation for acquisition of the exposure
    /// time. Returns true if the camera is in manual exposure mode, otherwise
    /// false.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn is_exposure_mode_manual(&self) -> Result<bool, Error> {
        let mut is_manual = false;
        unsafe {
            result_from(royale_sys::camera_get_exposure_mode(self.camera_ptr, &mut is_manual))?;
        }
        Ok(is_manual)
    }

    /// Changes the exposure mode for the supported operated operation modes.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn set_exposure_mode(&self, is_manual: bool) -> Result<(), Error> {
        unsafe { result_from(royale_sys::camera_set_exposure_mode(self.camera_ptr, is_manual)) }
    }

    /// Retrieves the minimum and maximum allowed exposure limits of the
    /// specified operation mode.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn get_exposure_limits(&self) -> Result<[u32; 2], Error> {
        let mut limits = [0; 2];
        unsafe {
            result_from(royale_sys::camera_get_exposure_limits(
                self.camera_ptr,
                &mut limits[0],
                &mut limits[1],
            ))?;
        }
        Ok(limits)
    }

    /// Changes the exposure time for the supported operated operation modes.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn set_exposure_time(&self, exposure_time: u32) -> Result<(), Error> {
        unsafe { result_from(royale_sys::camera_set_exposure_time(self.camera_ptr, exposure_time)) }
    }

    /// Begins streaming frames from the camera.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn capture_start(&self) -> Result<(), Error> {
        unsafe { result_from(royale_sys::camera_capture_start(self.camera_ptr)) }
    }

    /// Stops streaming frames from the camera.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub fn capture_stop(&self) -> Result<(), Error> {
        unsafe { result_from(royale_sys::camera_capture_stop(self.camera_ptr)) }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
        unsafe {
            royale_sys::camera_delete(self.camera_ptr, self.listener_ptr);
        }
    }
}
