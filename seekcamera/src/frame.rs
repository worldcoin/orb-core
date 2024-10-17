use crate::error::{result_from, Error};
use rkyv::{Archive, Deserialize, Serialize};
use seekcamera_sys::{
    seekcamera_frame_format_t_SEEKCAMERA_FRAME_FORMAT_GRAYSCALE,
    seekcamera_frame_get_frame_by_format, seekcamera_frame_t, seekframe_get_data,
    seekframe_get_height, seekframe_get_width,
};
use std::{
    fmt,
    ops::Deref,
    ptr,
    time::{Duration, SystemTime},
};

/// Seek thermal camera frame.
#[derive(Clone, Archive, Serialize, Deserialize)]
pub struct Frame {
    data: Vec<u8>,
    timestamp: Duration,
    width: usize,
    height: usize,
}

/// Rotation method for thermal camera frame
#[derive(Clone)]
pub enum Rotation {
    /// Rotate the captured frame clockwise.
    Clockwise,
    /// Rotate the captured frame counter-clockwise.j
    CounterClockwise,
}

impl Deref for Frame {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("timestamp", &self.timestamp)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Frame {
    /// Copies a frame from the frame storage. Performs rotation clockwise.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    ///
    /// # Safety
    ///
    /// `frame` must be valid for the lifetime of the function.
    pub unsafe fn obtain(
        frame: *mut seekcamera_frame_t,
        rotation: &Rotation,
    ) -> Result<Self, Error> {
        unsafe {
            let mut frame_ptr = ptr::null_mut();
            result_from(seekcamera_frame_get_frame_by_format(
                frame,
                seekcamera_frame_format_t_SEEKCAMERA_FRAME_FORMAT_GRAYSCALE,
                &mut frame_ptr,
            ))?;
            let data = seekframe_get_data(frame_ptr);
            let width = seekframe_get_width(frame_ptr);
            let height = seekframe_get_height(frame_ptr);
            let mut rotated = vec![0; width * height];
            match rotation {
                Rotation::Clockwise => {
                    copy_rotated_cw(data.cast(), rotated.as_mut_ptr(), width, height);
                }
                Rotation::CounterClockwise => {
                    copy_rotated_ccw(data.cast(), rotated.as_mut_ptr(), width, height);
                }
            }
            Ok(Self {
                data: rotated,
                timestamp: SystemTime::UNIX_EPOCH.elapsed().unwrap_or(Duration::MAX),
                width: height,
                height: width,
            })
        }
    }

    /// Creates a new frame from raw data.
    #[must_use]
    pub fn new(data: Vec<u8>, timestamp: Duration, width: usize, height: usize) -> Self {
        Self { data, timestamp, width, height }
    }

    /// Returns the frame data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns the frame timestamp.
    #[must_use]
    pub fn timestamp(&self) -> Duration {
        self.timestamp
    }

    /// Returns the sensor frame width.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Returns the sensor frame height.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }
}

fn copy_rotated_cw(mut src: *const u8, dst: *mut u8, width: usize, height: usize) {
    unsafe {
        for x in 0..height {
            let mut dst_row = dst.add(height - 1 - x);
            for _ in 0..width {
                *dst_row = *src;
                dst_row = dst_row.add(height);
                src = src.add(1);
            }
        }
    }
}

fn copy_rotated_ccw(mut src: *const u8, dst: *mut u8, width: usize, height: usize) {
    unsafe {
        let dst_end = dst.add(width * height);
        for x in 0..height {
            let mut dst_row = dst_end.sub(height - x);
            for _ in 0..width {
                *dst_row = *src;
                dst_row = dst_row.sub(height);
                src = src.add(1);
            }
        }
    }
}
