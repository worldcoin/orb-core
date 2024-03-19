//! `rkyv` support for `ndarray` types.

#![allow(clippy::used_underscore_binding)] // triggered by rkyv

use crate::agents::camera;
use ndarray::prelude::*;
use rkyv::{Archive, Deserialize, Serialize};
use serde::ser::SerializeStruct;
use std::{marker::PhantomData, mem::transmute, time::SystemTime};

/// Ndarray wrapper which can be archived by `rkyv`.
#[derive(Clone, Debug, Default, Archive, Serialize, Deserialize)]
pub struct RkyvNdarray<A, D> {
    vec: Vec<A>,
    shape: Box<[usize]>,
    strides: Box<[isize]>,
    _marker: PhantomData<D>,
}

impl<A: serde::Serialize, D> serde::Serialize for RkyvNdarray<A, D> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("RkyvNdarray", 3)?;
        state.serialize_field("vec", &self.vec)?;
        state.serialize_field("shape", &self.shape)?;
        state.serialize_field("strides", &self.strides)?;
        state.end()
    }
}

macro_rules! impl_rkyv_ndarray {
    ($ix:ident, $n:literal) => {
        impl<A> From<Array<A, $ix>> for RkyvNdarray<A, $ix> {
            fn from(array: Array<A, $ix>) -> Self {
                let shape = Box::from(array.shape());
                let strides = Box::from(array.strides());
                let vec = array.into_raw_vec();
                Self { vec, shape, strides, _marker: PhantomData }
            }
        }

        impl<A> RkyvNdarray<A, $ix> {
            /// Converts to an owned array.
            #[must_use]
            pub fn into_ndarray(self) -> Array<A, $ix> {
                let Self { vec, shape, strides, _marker } = self;
                let shape: [usize; $n] = shape.as_ref().try_into().unwrap();
                let strides: [isize; $n] = strides.as_ref().try_into().unwrap();
                Array::from_shape_vec(
                    shape.strides(unsafe { transmute::<[isize; $n], [usize; $n]>(strides) }),
                    vec,
                )
                .unwrap()
            }

            /// Returns an array view for this array.
            #[must_use]
            pub fn as_ndarray(&self) -> ArrayView<A, $ix> {
                let shape: [usize; $n] = self.shape.as_ref().try_into().unwrap();
                let strides: [isize; $n] = self.strides.as_ref().try_into().unwrap();
                ArrayView::from_shape(
                    shape.strides(unsafe { transmute::<[isize; $n], [usize; $n]>(strides) }),
                    &self.vec,
                )
                .unwrap()
            }
        }
    };
}

impl From<RkyvNdarray<u8, ndarray::Dim<[usize; 3]>>> for camera::rgb::Frame {
    fn from(ndarry: RkyvNdarray<u8, ndarray::Dim<[usize; 3]>>) -> Self {
        let height = ndarry.shape[0].try_into().expect("height to fit in u32");
        let width = ndarry.shape[1].try_into().expect("width to fit in u32");
        assert!(ndarry.shape[2] == 3, "Expected 3 channels");

        let t = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time must be after UNIX EPOCH");

        camera::rgb::Frame::from_vec(ndarry.vec, t, width, height)
    }
}

impl_rkyv_ndarray!(Ix0, 0);
impl_rkyv_ndarray!(Ix1, 1);
impl_rkyv_ndarray!(Ix2, 2);
impl_rkyv_ndarray!(Ix3, 3);
impl_rkyv_ndarray!(Ix4, 4);
impl_rkyv_ndarray!(Ix5, 5);
impl_rkyv_ndarray!(Ix6, 6);
