//! WLD Data ID - An ID format for Orb signups and images

#![warn(missing_docs, unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

mod s3_region;
mod wld_data_id;

pub use self::{
    s3_region::S3Region,
    wld_data_id::{ImageId, SignupId},
};
