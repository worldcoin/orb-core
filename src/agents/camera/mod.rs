//! A common frame trait.

pub mod ir;
pub mod rgb;
pub mod thermal;

use super::{Agent, AgentProcess, AgentTask, AgentThread};
use png::EncodingError;
use serde::{Deserialize, Serialize};
use std::{io::Write, time::Duration};

/// Control image frame resolution factor.
#[derive(Default, Copy, Clone, Debug)]
pub enum FrameResolution {
    /// Provides the maximum available resolution.
    #[default]
    MAX = 1,
    /// Provides half the maximum resolution.
    MEDIUM = 2,
    /// Provides 1/4 the maximum resolution.
    LOW = 4,
}

/// A common image frame interface.
pub trait Frame: Clone {
    /// Encodes the frame data as a PNG image with a scale down resolution
    /// factor.
    fn write_png<W: Write>(
        &self,
        writer: W,
        resolution: FrameResolution,
    ) -> Result<(), EncodingError>;

    /// Returns the frame timestamp.
    fn timestamp(&self) -> Duration;

    /// Returns the sensor frame width.
    fn width(&self) -> u32;

    /// Returns the sensor frame height.
    fn height(&self) -> u32;
}

/// Camera State.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Camera in idle.
    Idle,
    /// Camera is currently capturing.
    Capturing,
    /// Camera has some error.
    Error,
}

//-----------------------//
// ---- Helper code ---- //
//-----------------------//

#[allow(clippy::uninit_vec)]
fn frame_flip<T, U, F>(src: &[T], width: usize, height: usize, tf: F) -> Vec<U>
where
    F: Fn(T) -> U,
    T: Copy,
{
    let mut dst = Vec::<U>::with_capacity(width * height);
    unsafe {
        dst.set_len(dst.capacity());
        let mut src_ptr = src.as_ptr();
        let mut dst_ptr = dst.as_mut_ptr().sub(width);
        for _ in 0..height {
            dst_ptr = dst_ptr.add(width * 2);
            for _ in 0..width {
                let px = tf(*src_ptr);
                src_ptr = src_ptr.add(1);
                dst_ptr = dst_ptr.sub(1);
                *dst_ptr = px;
            }
        }
    }
    dst
}

#[allow(clippy::uninit_vec)]
fn frame_rotate_cw<T, U, F>(src: &[T], width: usize, height: usize, tf: F) -> Vec<U>
where
    F: Fn(T) -> U,
    T: Copy,
{
    let mut dst = Vec::<U>::with_capacity(width * height);
    unsafe {
        dst.set_len(dst.capacity());
        let mut src_ptr = src.as_ptr();
        for x in 0..height {
            let mut dst_ptr = dst.as_mut_ptr().add(height - 1 - x);
            for _ in 0..width {
                *dst_ptr = tf(*src_ptr);
                dst_ptr = dst_ptr.add(height);
                src_ptr = src_ptr.add(1);
            }
        }
    }
    dst
}

#[allow(clippy::uninit_vec)]
fn frame_rotate_cw_flip<T, U, F>(src: &[T], width: usize, height: usize, tf: F) -> Vec<U>
where
    F: Fn(T) -> U,
    T: Copy,
{
    let mut dst = Vec::<U>::with_capacity(width * height);
    unsafe {
        dst.set_len(dst.capacity());
        let mut src_ptr = src.as_ptr();
        for x in 0..height {
            let mut dst_ptr = dst.as_mut_ptr().add(x);
            for _ in 0..width {
                *dst_ptr = tf(*src_ptr);
                dst_ptr = dst_ptr.add(height);
                src_ptr = src_ptr.add(1);
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::{frame_flip, frame_rotate_cw, frame_rotate_cw_flip};

    #[test]
    fn test_frame_transformations() {
        let input = vec![1, 2, 3, 4, 5, 6];

        {
            let output = frame_rotate_cw(&input, 3, 2, std::convert::identity);
            let answer = vec![4, 1, 5, 2, 6, 3];
            assert_eq!(output, answer);
        }

        {
            let output = frame_rotate_cw_flip(&input, 3, 2, std::convert::identity);
            let answer = vec![1, 4, 2, 5, 3, 6];
            assert_eq!(output, answer);
        }

        {
            let output = frame_flip(&input, 3, 2, std::convert::identity);
            let answer = vec![3, 2, 1, 6, 5, 4];
            assert_eq!(output, answer);
        }

        {
            let output = frame_rotate_cw(&input, 3, 2, |x| x + 1);
            let answer = vec![5, 2, 6, 3, 7, 4];
            assert_eq!(output, answer);
        }
    }
}
