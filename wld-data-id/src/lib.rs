//! WLD Data ID - An ID format for Orb signups and images
//!
//! Spec: <https://docs.google.com/document/d/1cTvZUwkAwECzB346Y3o6kZx7vF20W1VNTAQ9PZlSA-8>

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod s3_region;
mod wld_data_id;

pub use self::{
    s3_region::S3Region,
    wld_data_id::{ImageId, SignupId},
};
