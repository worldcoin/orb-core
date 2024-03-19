//! Seek Thermal camera interface.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

mod camera;
mod error;
mod frame;

pub use camera::{AttachError, Camera, RecvError};
pub use error::Error;
pub use frame::Frame;

use error::result_from;
use seekcamera_sys::{
    seekcamera_error_t, seekcamera_frame_t, seekcamera_manager_event_t,
    seekcamera_manager_register_event_callback, seekcamera_manager_t,
    seekcamera_register_frame_available_callback, seekcamera_t,
};
use std::os::raw::c_void;

type EventCallbackClosure = Box<dyn Fn(*mut seekcamera_t, seekcamera_manager_event_t)>;
type FrameCallbackClosure = Box<dyn Fn(*mut seekcamera_t, *mut seekcamera_frame_t)>;

// NOTE for simplicity, the closure will be leaked forever.
unsafe fn register_event_callback(
    camera_manager: *mut seekcamera_manager_t,
    closure: EventCallbackClosure,
) -> Result<(), Error> {
    extern "C" fn event_callback(
        camera: *mut seekcamera_t,
        event: seekcamera_manager_event_t,
        event_status: seekcamera_error_t,
        user_data: *mut c_void,
    ) {
        match result_from(event_status) {
            Ok(()) | Err(Error::NotPaired) => unsafe {
                (*user_data.cast::<EventCallbackClosure>())(camera, event);
            },
            Err(err) => {
                log::error!("Seek camera manager error: {err}");
            }
        }
    }
    let closure = Box::into_raw(Box::new(closure));
    unsafe {
        let result = result_from(seekcamera_manager_register_event_callback(
            camera_manager,
            Some(event_callback),
            closure.cast(),
        ));
        if let Err(err) = result {
            drop(Box::from_raw(closure));
            log::error!("Couldn't register seek camera manager event callback: {err}");
        }
        result
    }
}

// NOTE for simplicity, the closure will be leaked forever.
unsafe fn register_frame_callback(
    camera: *mut seekcamera_t,
    closure: FrameCallbackClosure,
) -> Result<(), Error> {
    extern "C" fn frame_callback(
        camera: *mut seekcamera_t,
        frame: *mut seekcamera_frame_t,
        user_data: *mut c_void,
    ) {
        unsafe { (*user_data.cast::<FrameCallbackClosure>())(camera, frame) };
    }
    let closure = Box::into_raw(Box::new(closure));
    unsafe {
        let result = result_from(seekcamera_register_frame_available_callback(
            camera,
            Some(frame_callback),
            closure.cast(),
        ));
        if let Err(err) = result {
            drop(Box::from_raw(closure));
            log::error!("Couldn't register seek camera frame callback: {err}");
        }
        result
    }
}
