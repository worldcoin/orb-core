#![cfg_attr(not(all(target_arch = "aarch64", target_os = "linux")), allow(unused_imports))]

use std::{cmp::min, fmt, mem::MaybeUninit, ops::Deref, time::Duration};

/// Royale SDK frame.
#[derive(Clone)]
pub struct Frame {
    points: Vec<DepthPoint>,
    gray: Vec<u16>,
    timestamp: Duration,
    width: u16,
    height: u16,
}

/// 3D point in object space, with coordinates in meters.
#[derive(Clone, Default, Debug)]
pub struct DepthPoint {
    /// X coordinate in meters.
    pub x: f32,
    /// Y coordinate in meters.
    pub y: f32,
    /// Z coordinate in meters.
    pub z: f32,
    /// noise value in meters.
    pub noise: f32,
    /// value from 0 (invalid) to 255 (full confidence).
    pub depth_confidence: u8,
}

impl Deref for Frame {
    type Target = [DepthPoint];

    fn deref(&self) -> &Self::Target {
        &self.points
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
    /// Copies a frame from Royale SDK. Performs rotation counter-clockwise.
    ///
    /// # Errors
    ///
    /// This method can result in a generic [`Error`].
    ///
    /// # Safety
    ///
    /// `frame` must be valid for the lifetime of the function.
    #[must_use]
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    pub unsafe fn obtain(frame: *const royale_sys::Frame) -> Self {
        let (mut width, mut height) = (0, 0);
        let mut timestamp = 0;
        unsafe { royale_sys::frame_metadata(frame, &mut width, &mut height, &mut timestamp) };
        let total = usize::from(width * height);
        let mut points = Vec::with_capacity(total);
        let mut gray = Vec::with_capacity(total);
        for i in 0..usize::from(width) {
            for j in 0..usize::from(height) {
                let mut x = MaybeUninit::uninit();
                let mut y = MaybeUninit::uninit();
                let mut z = MaybeUninit::uninit();
                let mut noise = MaybeUninit::uninit();
                let mut gray_value = MaybeUninit::uninit();
                let mut depth_confidence = MaybeUninit::uninit();
                unsafe {
                    royale_sys::frame_point(
                        frame,
                        (usize::from(width) - 1 - i) + usize::from(width) * j,
                        x.as_mut_ptr(),
                        y.as_mut_ptr(),
                        z.as_mut_ptr(),
                        noise.as_mut_ptr(),
                        gray_value.as_mut_ptr(),
                        depth_confidence.as_mut_ptr(),
                    );
                    points.push(DepthPoint {
                        x: x.assume_init(),
                        y: y.assume_init(),
                        z: z.assume_init(),
                        noise: noise.assume_init(),
                        depth_confidence: depth_confidence.assume_init(),
                    });
                    gray.push(gray_value.assume_init());
                }
            }
        }
        Self {
            points,
            gray,
            timestamp: Duration::from_micros(timestamp),
            width: height,
            height: width,
        }
    }

    /// Creates a new frame from raw data.
    #[must_use]
    pub fn new(
        points: Vec<DepthPoint>,
        gray: Vec<u16>,
        timestamp: Duration,
        width: u16,
        height: u16,
    ) -> Self {
        Self { points, gray, timestamp, width, height }
    }

    /// Returns a slice of 16-bit gray values.
    #[must_use]
    pub fn as_gray(&self) -> &[u16] {
        &self.gray
    }

    /// Returns the frame timestamp.
    #[must_use]
    pub fn timestamp(&self) -> Duration {
        self.timestamp
    }

    /// Returns the frame width.
    #[must_use]
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Returns the frame height.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.height
    }
}

impl DepthPoint {
    /// Maps the depth point into the RGB color space.
    ///
    /// Color mapping:
    /// 1. Red - close distance
    /// 2. Yellow - middle distance
    /// 3. Green - far distance
    /// 4. Black - low confidence or high noise
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    #[must_use]
    pub fn to_rgb(&self) -> [u8; 3] {
        const MIN_DISTANCE: f32 = 0.2;
        const MAX_DISTANCE: f32 = 0.6;
        let distance = self.z.clamp(MIN_DISTANCE, MAX_DISTANCE);
        let normalized = (distance - MIN_DISTANCE) / (MAX_DISTANCE - MIN_DISTANCE);
        let value = min(
            self.depth_confidence,
            u8::MAX - ((self.noise / MAX_DISTANCE).min(1.0) * f32::from(u8::MAX)) as u8,
        );
        if normalized < 0.5 {
            [value, min(value, (normalized * 2.0 * f32::from(u8::MAX)) as u8), 0]
        } else {
            [min(value, u8::MAX - ((normalized - 0.5) * 2.0 * f32::from(u8::MAX)) as u8), value, 0]
        }
    }
}
