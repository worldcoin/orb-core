use crate::{
    error::{result_from, Error},
    frame::Rotation,
    register_event_callback, register_frame_callback, Frame,
};
use seekcamera_sys::{
    seekcamera_capture_session_start, seekcamera_capture_session_stop,
    seekcamera_flat_scene_correction_id_t_SEEKCAMERA_FLAT_SCENE_CORRECTION_ID_0,
    seekcamera_frame_format_t_SEEKCAMERA_FRAME_FORMAT_GRAYSCALE,
    seekcamera_io_type_t_SEEKCAMERA_IO_TYPE_USB, seekcamera_manager_create,
    seekcamera_manager_destroy, seekcamera_manager_t, seekcamera_store_calibration_data,
    seekcamera_store_flat_scene_correction, seekcamera_t,
};
use std::{
    ptr,
    sync::mpsc,
    time::{Duration, Instant},
};

type EventRx = mpsc::Receiver<(*mut seekcamera_t, Event)>;
type FrameRx = mpsc::Receiver<(*mut seekcamera_t, Result<Frame, Error>)>;

/// Seek Thermal camera interface.
pub struct Camera {
    camera_manager: *mut seekcamera_manager_t,
    camera: *mut seekcamera_t,
    event_rx: EventRx,
    frame_rx: FrameRx,
}

/// Error returned from [`Camera::attach`].
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// Seek camera manager creation error.
    #[error("seek camera manager creation error: {}", .0)]
    ManagerCreate(Error),
    /// Seek camera event callback registration error.
    #[error("seek camera event callback registration error: {}", .0)]
    RegisterEventCallback(Error),
    /// Seek camera frame callback registration error.
    #[error("seek camera frame callback registration error: {}", .0)]
    RegisterFrameCallback(Error),
    /// Seek camera disconnected.
    #[error("seek camera disconnected")]
    Disconnected,
    /// Seek camera has an error.
    #[error("seek camera has an error")]
    CameraError,
    /// Seek camera pairing error.
    #[error("seek camera pairing error: {}", .0)]
    Pairing(Error),
    /// Seek camera connection timeout.
    #[error("seek camera connection timeout")]
    Timeout,
}

/// Error returned from [`Camera::recv`].
#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    /// Seek camera disconnected.
    #[error("seek camera disconnected")]
    Disconnected,
    /// Seek camera frame error.
    #[error("seek camera frame error: {}", .0)]
    Frame(Error),
}

#[derive(Debug)]
enum Event {
    Connect,
    Disconnect,
    Error,
    ReadyToPair,
}

impl Camera {
    /// Attaches to a Seek Thermal camera connected via USB. If there are
    /// multiple cameras, this method connects to the first one that emits the
    /// connection event. This method runs the pairing procedure if needed.
    ///
    /// # Errors
    ///
    /// See [`AttachError`] for all possible errors.
    pub fn attach(connection_timeout: Duration, rotation: Rotation) -> Result<Self, AttachError> {
        let camera_manager = manager_create().map_err(AttachError::ManagerCreate)?;
        let event_rx = unsafe {
            make_event_channel(camera_manager).map_err(AttachError::RegisterEventCallback)?
        };
        let camera = unsafe { camera_connect(&event_rx, connection_timeout)? };
        let frame_rx = unsafe {
            make_frame_channel(camera, rotation).map_err(AttachError::RegisterFrameCallback)?
        };
        Ok(Self { camera_manager, camera, event_rx, frame_rx })
    }

    /// Attempts to wait for a frame from this camera.
    ///
    /// # Errors
    ///
    /// See [`RecvError`] for all possible errors.
    #[allow(clippy::missing_panics_doc)]
    pub fn recv(&self) -> Result<Frame, RecvError> {
        match self.event_rx.try_recv() {
            Ok((_, Event::Disconnect)) => return Err(RecvError::Disconnected),
            Ok((_, Event::Error)) => log::error!("Seek thermal camera error"),
            Ok((_, event @ (Event::Connect | Event::ReadyToPair))) => {
                log::warn!("Unexpected seek thermal camera event: {event:?}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => unreachable!(),
        }
        let (_camera, frame) = self.frame_rx.recv().unwrap();
        frame.map_err(RecvError::Frame)
    }

    /// Begins streaming frames of the grayscale output format from the camera.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    pub fn capture_start(&self) -> Result<(), Error> {
        unsafe {
            result_from(seekcamera_capture_session_start(
                self.camera,
                seekcamera_frame_format_t_SEEKCAMERA_FRAME_FORMAT_GRAYSCALE,
            ))
        }
    }

    /// Stops streaming frames from the camera.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    pub fn capture_stop(&self) -> Result<(), Error> {
        unsafe { result_from(seekcamera_capture_session_stop(self.camera)) }
    }

    /// Stores a flat scene correction.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    pub fn store_flat_scene_correction(&self) -> Result<(), Error> {
        unsafe {
            result_from(seekcamera_store_flat_scene_correction(
                self.camera,
                seekcamera_flat_scene_correction_id_t_SEEKCAMERA_FLAT_SCENE_CORRECTION_ID_0,
                None,
                ptr::null_mut(),
            ))
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let result = unsafe { result_from(seekcamera_manager_destroy(&mut self.camera_manager)) };
        if let Err(err) = result {
            log::error!("Unexpectedly errored while destroying the seek camera: {err}");
        }
    }
}

fn manager_create() -> Result<*mut seekcamera_manager_t, Error> {
    let mut camera_manager = ptr::null_mut();
    unsafe {
        result_from(seekcamera_manager_create(
            &mut camera_manager,
            seekcamera_io_type_t_SEEKCAMERA_IO_TYPE_USB,
        ))?;
    }
    Ok(camera_manager)
}

unsafe fn camera_connect(
    event_rx: &mpsc::Receiver<(*mut seekcamera_t, Event)>,
    timeout: Duration,
) -> Result<*mut seekcamera_t, AttachError> {
    let deadline = Instant::now() + timeout;
    let (camera, event) = match event_rx.recv_timeout(deadline - Instant::now()) {
        Ok((camera, event)) => (camera, event),
        Err(mpsc::RecvTimeoutError::Timeout) => return Err(AttachError::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => unreachable!(),
    };
    match event {
        Event::Connect => {
            log::info!("Seek thermal camera connected");
            return Ok(camera);
        }
        Event::Disconnect => return Err(AttachError::Disconnected),
        Event::Error => return Err(AttachError::CameraError),
        Event::ReadyToPair => log::warn!("Pairing seek thermal camera"),
    }
    let result = unsafe {
        result_from(seekcamera_store_calibration_data(camera, ptr::null(), None, ptr::null_mut()))
    };
    match result {
        Ok(()) => Ok(camera),
        Err(err) => Err(AttachError::Pairing(err)),
    }
}

unsafe fn make_event_channel(camera_manager: *mut seekcamera_manager_t) -> Result<EventRx, Error> {
    let (event_tx, event_rx) = mpsc::channel();
    // NOTE the callback will be leaked when the camera interface is dropped.
    // Normally we should keep the interface open forever.
    let callback = move |camera, event| {
        let event = match event {
            seekcamera_sys::seekcamera_manager_event_t_SEEKCAMERA_MANAGER_EVENT_CONNECT => {
                Event::Connect
            }
            seekcamera_sys::seekcamera_manager_event_t_SEEKCAMERA_MANAGER_EVENT_DISCONNECT => {
                Event::Disconnect
            }
            seekcamera_sys::seekcamera_manager_event_t_SEEKCAMERA_MANAGER_EVENT_ERROR => {
                Event::Error
            }
            seekcamera_sys::seekcamera_manager_event_t_SEEKCAMERA_MANAGER_EVENT_READY_TO_PAIR => {
                Event::ReadyToPair
            }
            event => {
                log::error!("Unknown seek camera event occured: {event}");
                return;
            }
        };
        let _ = event_tx.send((camera, event));
    };
    unsafe { register_event_callback(camera_manager, Box::new(callback))? };
    Ok(event_rx)
}

unsafe fn make_frame_channel(
    camera: *mut seekcamera_t,
    rotation: Rotation,
) -> Result<FrameRx, Error> {
    let (frame_tx, frame_rx) = mpsc::channel();
    // NOTE the callback will be leaked when the camera interface is dropped.
    // Normally we should keep the interface open forever.
    let callback = move |camera, frame| {
        let frame = unsafe { Frame::obtain(frame, &rotation) };
        let _ = frame_tx.send((camera, frame));
    };
    unsafe { register_frame_callback(camera, Box::new(callback))? };
    Ok(frame_rx)
}
