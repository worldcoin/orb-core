use data_encoding::BASE64;
use ndarray::prelude::*;
use std::cmp::min;

/// Tensor representing iris or mask code.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Code {
    tensor: Array4<bool>,
}

impl From<Array4<bool>> for Code {
    fn from(tensor: Array4<bool>) -> Self {
        Self { tensor }
    }
}

impl AsRef<Array4<bool>> for Code {
    fn as_ref(&self) -> &Array4<bool> {
        &self.tensor
    }
}

impl Code {
    /// Serializes the code to a base64-encoded packed array with the given
    /// `rotation`.
    #[must_use]
    pub fn to_packed_base64(&self, rotation: isize) -> String {
        let rolled = roll_1(&self.tensor, rotation);
        let packed = pack_bits(&rolled);
        BASE64.encode(&packed)
    }
}

// Roll array elements along the second axis.
//
// Analogue of `numpy.roll(array, shift, axis=1)`.
#[must_use]
fn roll_1(array: &Array4<bool>, shift: isize) -> Array4<bool> {
    let mut rolled = array.clone();
    if shift != 0 {
        let dst_r = s![.., shift.., .., ..];
        let dst_l = s![.., ..shift, .., ..];
        let src_r = s![.., -shift.., .., ..];
        let src_l = s![.., ..-shift, .., ..];
        rolled.slice_mut(dst_r).assign(&array.slice(src_l));
        rolled.slice_mut(dst_l).assign(&array.slice(src_r));
    }
    rolled
}

// Packs the elements of a binary-valued array into bits in a `u8` array.
//
// Analogue of `numpy.packbits(array)`.
#[must_use]
fn pack_bits(array: &Array4<bool>) -> Vec<u8> {
    let mut packed = Vec::with_capacity(array.len() / 8 + min(array.len() % 8, 1));
    let mut byte = 0;
    let mut bit_counter = 0;
    for item in array {
        if *item {
            byte |= 1 << (7 - bit_counter);
        }
        bit_counter += 1;
        if bit_counter == 8 {
            packed.push(byte);
            byte = 0;
            bit_counter = 0;
        }
    }
    if bit_counter > 0 {
        packed.push(byte);
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::similar_names)]
    fn test_roll_1() {
        let shape = (2, 6, 1, 1);
        #[rustfmt::skip]
        let a_0 = Array::from_vec(vec![
            true, false, false, true, false, true,
            false, false, true, true, true, false,
        ]).into_shape(shape).unwrap();
        #[rustfmt::skip]
        let a_p1 = Array::from_vec(vec![
            true, true, false, false, true, false,
            false, false, false, true, true, true,
        ]).into_shape(shape).unwrap();
        #[rustfmt::skip]
        let a_p2 = Array::from_vec(vec![
            false, true, true, false, false, true,
            true, false, false, false, true, true,
        ]).into_shape(shape).unwrap();
        #[rustfmt::skip]
        let a_n1 = Array::from_vec(vec![
            false, false, true, false, true, true,
            false, true, true, true, false, false,
        ]).into_shape(shape).unwrap();
        #[rustfmt::skip]
        let a_n2 = Array::from_vec(vec![
            false, true, false, true, true, false,
            true, true, true, false, false, false,
        ]).into_shape(shape).unwrap();
        assert_eq!(roll_1(&a_0, 0), a_0);
        assert_eq!(roll_1(&a_0, 1), a_p1);
        assert_eq!(roll_1(&a_0, 2), a_p2);
        assert_eq!(roll_1(&a_0, -1), a_n1);
        assert_eq!(roll_1(&a_0, -2), a_n2);
    }

    #[test]
    fn test_pack_bits() {
        assert_eq!(
            pack_bits(&Array::from_vec(vec![false]).into_shape((1, 1, 1, 1)).unwrap()),
            vec![0]
        );
        assert_eq!(
            pack_bits(&Array::from_vec(vec![true]).into_shape((1, 1, 1, 1)).unwrap()),
            vec![0b1000_0000]
        );
        assert_eq!(
            pack_bits(
                &Array::from_vec(vec![false, true, true, false, true, false, false, true])
                    .into_shape((2, 2, 2, 1))
                    .unwrap()
            ),
            vec![0b0110_1001]
        );
        assert_eq!(
            pack_bits(
                &Array::from_vec(vec![
                    true, false, true, false, true, true, true, false, false, true, true, false,
                    true, false, false, true, false, false, false
                ])
                .into_shape((19, 1, 1, 1))
                .unwrap()
            ),
            vec![0b1010_1110, 0b0110_1001, 0b0000_0000]
        );
    }
}
