//! Eye tracker agent.

use crate::{
    agents::{mirror, python},
    identification,
    image::fisheye::{self, Fisheye},
    pid::{derivative::LowPassFilter, InstantTimer, Timer},
};
use agentwire::port::{self, Port};
use eyre::{Error, Result, WrapErr};
use futures::prelude::*;
use std::f64::consts::PI;
use tokio::runtime;

/// Diamond B3
const OFFSET_RGB_CAMERA_TO_MIRROR_X_DIAMOND_MM: f64 = 23.92;
const OFFSET_RGB_CAMERA_TO_MIRROR_Y_DIAMOND_MM: f64 = 0.71;
const OFFSET_RGB_CAMERA_TO_MIRROR_Z_DIAMOND_MM: f64 = -55.60;

const OFFSET_RGB_CAMERA_TO_MIRROR_X_PEARL_MM: f64 = 27.4;
const OFFSET_RGB_CAMERA_TO_MIRROR_Y_PEARL_MM: f64 = 8.2;
const OFFSET_RGB_CAMERA_TO_MIRROR_Z_PEARL_MM: f64 = -44.39;

const RGB_CAMERA_VIEW_ANGLE_HORIZONTAL_DEGREES: f64 = 73.568;
const RGB_CAMERA_VIEW_ANGLE_VERTICAL_DEGREES: f64 = 94.382;
const LOW_PASS_FILTER_RC: f64 = 0.16;

/// Eye tracker agent.
///
/// See [the module-level documentation](self) for details.
#[derive(Debug)]
pub struct Agent {
    /// Horizontal camera view angle multiplier due to undistortion.
    horizontal_multiplier: f64,
    /// Vertical camera view angle multiplier due to undistortion.
    vertical_multiplier: f64,
}

/// Agent input.
#[derive(Clone, Debug)]
pub enum Input {
    /// Set fisheye configuration.
    Fisheye(Option<fisheye::Config>),
    /// RGB-Net estimate data.
    Track {
        /// Target the left eye if `true` or the right eye if `false`.
        target_left_eye: bool,
        /// Left eye distorted horizontal coordinate.
        distorted_left_x: f64,
        /// Left eye distorted vertical coordinate.
        distorted_left_y: f64,
        /// Right eye distorted horizontal coordinate.
        distorted_right_x: f64,
        /// Right eye distorted vertical coordinate.
        distorted_right_y: f64,
        /// Estimated user distance.
        user_distance: f64,
    },
    /// Resets the agent.
    Reset,
}

impl Port for Agent {
    type Input = Input;
    type Output = mirror::Point;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "eye-tracker";
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
impl agentwire::agent::Thread for Agent {
    type Error = Error;

    fn run(mut self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let offset_rgb_camera_to_mirror_y_mm;
        let offset_rgb_camera_to_mirror_z_mm;
        let offset_rgb_camera_to_mirror_x_mm;

        if identification::HARDWARE_VERSION.contains("Diamond") {
            #[cfg(feature = "debug-eye-tracker")]
            println!("\rThis is a Diamond Orb");
            offset_rgb_camera_to_mirror_y_mm = OFFSET_RGB_CAMERA_TO_MIRROR_Y_DIAMOND_MM;
            offset_rgb_camera_to_mirror_z_mm = OFFSET_RGB_CAMERA_TO_MIRROR_Z_DIAMOND_MM;
            offset_rgb_camera_to_mirror_x_mm = OFFSET_RGB_CAMERA_TO_MIRROR_X_DIAMOND_MM;
        } else {
            #[cfg(feature = "debug-eye-tracker")]
            println!("\rThis is a Pearl Orb");
            offset_rgb_camera_to_mirror_y_mm = OFFSET_RGB_CAMERA_TO_MIRROR_Y_PEARL_MM;
            offset_rgb_camera_to_mirror_z_mm = OFFSET_RGB_CAMERA_TO_MIRROR_Z_PEARL_MM;
            offset_rgb_camera_to_mirror_x_mm = OFFSET_RGB_CAMERA_TO_MIRROR_X_PEARL_MM;
        }

        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        let mut timer = InstantTimer::default();
        let mut filter_left_x = LowPassFilter::default();
        let mut filter_left_y = LowPassFilter::default();
        let mut filter_right_x = LowPassFilter::default();
        let mut filter_right_y = LowPassFilter::default();
        let mut fisheye = None;
        while let Some(input) = rt.block_on(port.next()) {
            let output = match input.value {
                Input::Fisheye(fisheye_config) => {
                    fisheye = fisheye_config
                        .map(Fisheye::try_from)
                        .transpose()
                        .wrap_err("failed constructing fisheye from fisheye config")?;
                    let coordinates = fisheye.as_ref().unwrap().undistort_coordinates(vec![
                        (0.5, 1.0),
                        (0.5, 0.0),
                        (1.0, 0.5),
                        (0.0, 0.5),
                    ])?;
                    self.horizontal_multiplier = f64::from(coordinates[0].1 - coordinates[1].1);
                    self.vertical_multiplier = f64::from(coordinates[2].0 - coordinates[3].0);
                    continue;
                }
                Input::Track {
                    target_left_eye,
                    distorted_left_x,
                    distorted_left_y,
                    distorted_right_x,
                    distorted_right_y,
                    user_distance,
                } => {
                    if distorted_left_x.is_nan()
                        || distorted_left_y.is_nan()
                        || distorted_right_x.is_nan()
                        || distorted_right_y.is_nan()
                    {
                        continue;
                    }
                    if user_distance.is_nan() {
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    let [(left_x, left_y), (right_x, right_y)]: [(f32, f32);
                        2] = fisheye
                        .as_ref()
                        .expect("fisheye to be initialized")
                        .undistort_coordinates(vec![
                            (distorted_left_x as f32, distorted_left_y as f32),
                            (distorted_right_x as f32, distorted_right_y as f32),
                        ])?
                        .try_into()
                        .unwrap();
                    #[cfg(feature = "debug-eye-tracker")]
                    {
                        println!("\rdistorted_right_x: {distorted_right_x}");
                        println!("\rright_x: {right_x}");
                    }
                    let dt = timer.get_dt().unwrap_or(0.0);
                    self.calculate_mirror_point(
                        target_left_eye,
                        filter_left_x.add(f64::from(left_x), dt, LOW_PASS_FILTER_RC),
                        filter_left_y.add(f64::from(left_y), dt, LOW_PASS_FILTER_RC),
                        filter_right_x.add(f64::from(right_x), dt, LOW_PASS_FILTER_RC),
                        filter_right_y.add(f64::from(right_y), dt, LOW_PASS_FILTER_RC),
                        user_distance,
                        offset_rgb_camera_to_mirror_y_mm,
                        offset_rgb_camera_to_mirror_z_mm,
                        offset_rgb_camera_to_mirror_x_mm,
                    )
                }
                Input::Reset => {
                    timer.reset();
                    filter_left_x.reset();
                    filter_left_y.reset();
                    filter_right_x.reset();
                    filter_right_y.reset();
                    continue;
                }
            };
            rt.block_on(async {
                port.send(input.chain(output)).await?;
                Ok::<_, Error>(())
            })?;
        }
        Ok(())
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self { horizontal_multiplier: 1.0, vertical_multiplier: 1.0 }
    }
}

#[allow(clippy::similar_names, clippy::too_many_arguments)]
impl Agent {
    fn calculate_mirror_point(
        &self,
        target_left_eye: bool,
        left_eye_x_position_percentage: f64,
        left_eye_y_position_percentage: f64,
        right_eye_x_position_percentage: f64,
        right_eye_y_position_percentage: f64,
        user_distance_mm: f64,
        offset_rgb_camera_to_mirror_y_mm: f64,
        offset_rgb_camera_to_mirror_z_mm: f64,
        offset_rgb_camera_to_mirror_x_mm: f64,
    ) -> mirror::Point {
        // notes:
        // the x axis in our RGB image is the y axis in our 3D coordinate system
        // the y axis in our RGB image is the z axis in our 3D coordinate system

        #[cfg(feature = "debug-eye-tracker")]
        {
            println!("\rcam_hor_mul: {}", self.horizontal_multiplier);
            println!("\rcam_vert_mul: {}", self.vertical_multiplier);
        }

        if target_left_eye {
            let left_eye_phi_degrees = calculate_gimbal_angle_phi_degrees(
                user_distance_mm,
                left_eye_x_position_percentage,
                RGB_CAMERA_VIEW_ANGLE_HORIZONTAL_DEGREES / self.horizontal_multiplier,
                offset_rgb_camera_to_mirror_y_mm,
                offset_rgb_camera_to_mirror_x_mm,
            );
            let left_eye_theta_degrees = calculate_gimbal_angle_theta_degrees(
                user_distance_mm,
                left_eye_y_position_percentage,
                RGB_CAMERA_VIEW_ANGLE_VERTICAL_DEGREES / self.vertical_multiplier,
                offset_rgb_camera_to_mirror_z_mm,
                offset_rgb_camera_to_mirror_x_mm,
            );
            #[cfg(feature = "debug-eye-tracker")]
            {
                println!("\ruser_distance_mm: {user_distance_mm}");
                println!("\rleft_x: {left_eye_x_position_percentage}");
                println!("\rleft_y: {left_eye_y_position_percentage}");
                println!("\rleft_eye_phi: {left_eye_phi_degrees}");
                println!("\rleft_eye_theta: {left_eye_theta_degrees}");
            }
            calculate_mirror_angles(left_eye_phi_degrees, left_eye_theta_degrees)
        } else {
            let right_eye_phi_degrees = calculate_gimbal_angle_phi_degrees(
                user_distance_mm,
                right_eye_x_position_percentage,
                RGB_CAMERA_VIEW_ANGLE_HORIZONTAL_DEGREES / self.horizontal_multiplier,
                offset_rgb_camera_to_mirror_y_mm,
                offset_rgb_camera_to_mirror_x_mm,
            );
            let right_eye_theta_degrees = calculate_gimbal_angle_theta_degrees(
                user_distance_mm,
                right_eye_y_position_percentage,
                RGB_CAMERA_VIEW_ANGLE_VERTICAL_DEGREES / self.vertical_multiplier,
                offset_rgb_camera_to_mirror_z_mm,
                offset_rgb_camera_to_mirror_x_mm,
            );
            #[cfg(feature = "debug-eye-tracker")]
            {
                println!("\ruser_distance_mm: {user_distance_mm}");
                println!("\rright_x: {right_eye_x_position_percentage}");
                println!("\rright_y: {right_eye_y_position_percentage}");
                println!("\rright_eye_phi: {right_eye_phi_degrees}");
                println!("\rright_eye_theta: {right_eye_theta_degrees}");
            }
            calculate_mirror_angles(right_eye_phi_degrees, right_eye_theta_degrees)
        }
    }
}

#[allow(clippy::similar_names)]
fn calculate_gimbal_angle_phi_degrees(
    user_distance_mm: f64,
    eye_position_y_percentage: f64,
    camera_view_angle_y_degrees: f64,
    offset_rgb_camera_to_mirror_y_mm: f64,
    offset_rgb_camera_to_mirror_x_mm: f64,
) -> f64 {
    // see gimbal_calculations.md for detailed explanation
    let camera_view_angle_rad: f64 = camera_view_angle_y_degrees / 180.0 * PI;
    let camera_view_range_mm: f64 = (camera_view_angle_rad / 2.0).tan() * user_distance_mm * 2.0;
    let gimbal_to_eye_distance_y_direction_mm: f64 = offset_rgb_camera_to_mirror_y_mm
        - camera_view_range_mm * eye_position_y_percentage
        + camera_view_range_mm / 2.0;
    #[cfg(feature = "debug-eye-tracker")]
    println!("\rgimbal_to_eye_distance_y_direction_mm: {gimbal_to_eye_distance_y_direction_mm}");
    let gimbal_angle_phi_rad: f64 = (gimbal_to_eye_distance_y_direction_mm
        / (user_distance_mm + offset_rgb_camera_to_mirror_x_mm))
        .atan();
    let gimbal_angle_phi_degrees: f64 = gimbal_angle_phi_rad / PI * 180.0;
    gimbal_angle_phi_degrees
}

#[allow(clippy::similar_names)]
fn calculate_gimbal_angle_theta_degrees(
    user_distance_mm: f64,
    eye_position_z_percentage: f64,
    camera_view_angle_z_degrees: f64,
    offset_rgb_camera_to_mirror_z_mm: f64,
    offset_rgb_camera_to_mirror_x_mm: f64,
) -> f64 {
    // see gimbal_calculations.md for detailed explanation
    let camera_view_angle_rad: f64 = camera_view_angle_z_degrees / 180.0 * PI;
    let camera_view_range_mm: f64 = (camera_view_angle_rad / 2.0).tan() * user_distance_mm * 2.0;
    let gimbal_to_eye_distance_z_direction_mm: f64 = -offset_rgb_camera_to_mirror_z_mm
        + camera_view_range_mm * eye_position_z_percentage
        - camera_view_range_mm / 2.0;
    #[cfg(feature = "debug-eye-tracker")]
    println!("\rgimbal_to_eye_distance_z_direction_mm: {gimbal_to_eye_distance_z_direction_mm}");
    let gimbal_angle_theta_rad: f64 = PI / 2.0
        + (gimbal_to_eye_distance_z_direction_mm
            / (user_distance_mm + offset_rgb_camera_to_mirror_x_mm))
            .atan();
    let gimbal_angle_theta_degrees: f64 = gimbal_angle_theta_rad / PI * 180.0;
    gimbal_angle_theta_degrees
}

fn calculate_mirror_angles(
    gimbal_angle_phi_degrees: f64,
    gimbal_angle_theta_degrees: f64,
) -> mirror::Point {
    // see gimbal_calculations.md for detailed explanation
    let gimbal_angle_phi_rad = gimbal_angle_phi_degrees / 180.0 * PI;
    let gimbal_angle_theta_rad = gimbal_angle_theta_degrees / 180.0 * PI;

    // transform gimbal vector from spherical to cartesian coordinates
    let gimbal_vector_x = gimbal_angle_theta_rad.sin() * gimbal_angle_phi_rad.cos();
    let gimbal_vector_y = gimbal_angle_theta_rad.sin() * gimbal_angle_phi_rad.sin();
    let gimbal_vector_z = gimbal_angle_theta_rad.cos();

    // calculate the normal vector of the mirror from the gimbal vector and the input vector (0 1 0)
    // and transform back to spherical coordinates
    let mirror_angle_phi_rad = ((1.0 + gimbal_vector_y) / gimbal_vector_x).atan();
    let mirror_angle_theta_rad = (gimbal_vector_z
        / (gimbal_vector_x.powi(2) + ((1.0 + gimbal_vector_y).powi(2)) + gimbal_vector_z.powi(2))
            .sqrt())
    .acos();

    let mirror_angle_phi_degrees = mirror_angle_phi_rad / PI * 180.0;
    let mirror_angle_theta_degrees = mirror_angle_theta_rad / PI * 180.0;

    #[cfg(feature = "debug-eye-tracker")]
    {
        println!("\rmirror_angle_phi: {mirror_angle_phi_degrees}");
        println!("\rmirror_angle_theta: {mirror_angle_theta_degrees}");
    }

    mirror::Point {
        phi_degrees: mirror_angle_phi_degrees,
        theta_degrees: mirror_angle_theta_degrees,
    }
}

impl Input {
    /// Creates a new agent input.
    #[must_use]
    pub fn track(
        target_left_eye: bool,
        rgb_net_estimate: &python::rgb_net::EstimateOutput,
    ) -> Option<Self> {
        let prediction = rgb_net_estimate.primary()?;
        Some(Self::Track {
            target_left_eye,
            distorted_left_x: prediction.landmarks.left_eye.x,
            distorted_left_y: prediction.landmarks.left_eye.y,
            distorted_right_x: prediction.landmarks.right_eye.x,
            distorted_right_y: prediction.landmarks.right_eye.y,
            user_distance: prediction.user_distance(),
        })
    }
}
