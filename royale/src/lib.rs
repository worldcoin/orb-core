//! Royale SDK interface.

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

mod camera;
mod error;
mod frame;

pub use camera::{AttachError, Camera};
pub use error::Error;
pub use frame::{DepthPoint, Frame};
