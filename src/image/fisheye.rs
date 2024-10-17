//! Fisheye compensation.

use crate::consts::{
    CONFIG_DIR, RGB_CALIBRATION_FILE, RGB_CALIBRATION_HEIGHT, RGB_CALIBRATION_WIDTH,
    RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH,
};
use eyre::{Error, Result};
use opencv::{
    calib3d::{get_optimal_new_camera_matrix, init_undistort_rectify_map, undistort_points},
    core::{no_array, Mat_AUTO_STEP, Point2f, Size, BORDER_CONSTANT, CV_16SC2, CV_8UC3},
    imgproc::{remap, INTER_LINEAR},
    prelude::*,
    types::VectorOfPoint2f,
};
use rkyv::{Archive, Deserialize, Serialize};
use std::{fs, path::Path, slice};

type DistCoeffs = [Vec<f64>; 1];
type CameraMatrix = [[f64; 3]; 3];

const DEFAULT_DIST_COEFFS: &[f64] = &[
    -1.638_357_483_957_033_2,
    1.153_244_417_311_696_2,
    0.0,
    0.0,
    -0.239_308_445_615_323_5,
    -1.657_585_963_003_335_2,
    1.174_019_539_639_798_,
    -0.235_123_438_390_057_06,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
];

const DEFAULT_CAMERA_MATRIX: CameraMatrix =
    [[314.021_381_905_447_3, 0.0, 239.5], [0.0, 314.021_381_905_447_3, 319.5], [0.0, 0.0, 1.0]];

/// Fisheye compensation model.
#[derive(Clone, Debug)]
pub struct Fisheye {
    /// Frame width.
    pub rgb_width: u32,
    /// Frame height.
    pub rgb_height: u32,
    camera_matrix: Mat,
    dist_coeffs: Mat,
    new_camera_matrix: Mat,
    map1: Mat,
    map2: Mat,
}

/// Fisheye configuration.
#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
pub struct Config {
    /// Resulting RGB frame width.
    pub rgb_width: u32,
    /// Resulting RGB frame height.
    pub rgb_height: u32,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Calibration {
    d: DistCoeffs,
    k: CameraMatrix,
}

impl Default for Config {
    fn default() -> Self {
        Self { rgb_width: RGB_REDUCED_WIDTH, rgb_height: RGB_REDUCED_HEIGHT }
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self { d: [Vec::from(DEFAULT_DIST_COEFFS)], k: DEFAULT_CAMERA_MATRIX }
    }
}

impl TryFrom<Config> for Fisheye {
    type Error = Error;

    #[allow(clippy::cast_possible_wrap)]
    fn try_from(Config { rgb_width, rgb_height }: Config) -> Result<Self> {
        let calibration_path = Path::new(CONFIG_DIR).join(RGB_CALIBRATION_FILE);
        let Calibration { d: dist_coeffs, k: mut camera_matrix } = calibration_path
            .exists()
            .then(|| fs::read_to_string(calibration_path))
            .transpose()?
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default();
        if rgb_width != RGB_CALIBRATION_WIDTH || rgb_height != RGB_CALIBRATION_HEIGHT {
            scale_camera_matrix(&mut camera_matrix, rgb_width, rgb_height);
        }
        let camera_matrix = make_camera_matrix(&camera_matrix)?;
        let dist_coeffs = make_dist_coeffs(&dist_coeffs)?;
        let image_size = Size { width: rgb_width as _, height: rgb_height as _ };
        let new_camera_matrix = get_optimal_new_camera_matrix(
            &camera_matrix,
            &dist_coeffs,
            image_size,
            1.0,
            image_size,
            None,
            false,
        )?;
        let mut map1 = Mat::default();
        let mut map2 = Mat::default();
        init_undistort_rectify_map(
            &camera_matrix,
            &dist_coeffs,
            &Mat::eye(3, 3, f32::opencv_type())?,
            &new_camera_matrix,
            image_size,
            CV_16SC2,
            &mut map1,
            &mut map2,
        )?;
        Ok(Self {
            rgb_width,
            rgb_height,
            camera_matrix,
            dist_coeffs,
            new_camera_matrix,
            map1,
            map2,
        })
    }
}

impl Fisheye {
    /// Undistorts coordinates on images with fisheye effect.
    #[allow(clippy::cast_precision_loss)]
    pub fn undistort_coordinates(&self, coordinates: Vec<(f32, f32)>) -> Result<Vec<(f32, f32)>> {
        let src = coordinates
            .into_iter()
            .map(|(x, y)| Point2f { x: x * self.rgb_width as f32, y: y * self.rgb_height as f32 })
            .collect::<VectorOfPoint2f>();
        let mut dst = VectorOfPoint2f::new();
        undistort_points(
            &src,
            &mut dst,
            &self.camera_matrix,
            &self.dist_coeffs,
            &no_array(),
            &self.new_camera_matrix,
        )?;
        Ok(dst
            .into_iter()
            .map(|Point2f { x, y }| (x / self.rgb_width as f32, y / self.rgb_height as f32))
            .collect())
    }

    /// Undistorts a fisheye image.
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    pub fn undistort_image(&self, data: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
        let src = unsafe {
            // This Mat must not outlive `data` slice.
            Mat::new_rows_cols_with_data(
                height as _,
                width as _,
                CV_8UC3,
                data.as_ptr() as *mut _,
                Mat_AUTO_STEP,
            )?
        };
        let mut dst = Mat::default();
        remap(&src, &mut dst, &self.map1, &self.map2, INTER_LINEAR, BORDER_CONSTANT, 0.into())?;
        assert!(dst.is_continuous());
        let len = dst
            .mat_size()
            .iter()
            .map(|&dim| dim as usize)
            .chain([dst.channels() as usize])
            .product();
        assert!(len == width as usize * height as usize * 3);
        let ptr = dst.ptr(0)?.cast::<u8>();
        let slice = unsafe { slice::from_raw_parts(ptr, len) };
        Ok(slice.to_vec())
    }
}

#[allow(clippy::cast_precision_loss)]
fn scale_camera_matrix(camera_matrix: &mut CameraMatrix, rgb_width: u32, rgb_height: u32) {
    check_aspect_ratio(rgb_width, rgb_height);
    let scale = f64::from(rgb_width) / f64::from(RGB_CALIBRATION_WIDTH);
    for row in &mut *camera_matrix {
        for elem in row {
            *elem *= scale;
        }
    }
    camera_matrix[2][2] = 1.0;
}

#[allow(clippy::cast_precision_loss)]
fn check_aspect_ratio(rgb_width: u32, rgb_height: u32) {
    const RGB_CALIBRATION_RATIO: f64 = RGB_CALIBRATION_WIDTH as f64 / RGB_CALIBRATION_HEIGHT as f64;
    let rgb_ratio = f64::from(rgb_width) / f64::from(rgb_height);
    assert!(
        (rgb_ratio - RGB_CALIBRATION_RATIO).abs() < 0.01,
        "rgb resolution ratio differs from calibration resolution ratio"
    );
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn make_camera_matrix(k: &CameraMatrix) -> Result<Mat> {
    let mut camera_matrix = Mat::new_rows_cols_with_default(3, 3, f32::opencv_type(), 0.0.into())?;
    for (i, row) in k.iter().enumerate() {
        for (j, &elem) in row.iter().enumerate() {
            *camera_matrix.at_2d_mut(i as _, j as _).unwrap() = elem as f32;
        }
    }
    Ok(camera_matrix)
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn make_dist_coeffs(d: &DistCoeffs) -> Result<Mat> {
    let mut dist_coeffs =
        Mat::new_rows_cols_with_default(1, d[0].len() as _, f32::opencv_type(), 0.0.into())?;
    for (i, &elem) in d[0].iter().enumerate() {
        *dist_coeffs.at_2d_mut(0, i as _).unwrap() = elem as f32;
    }
    Ok(dist_coeffs)
}
