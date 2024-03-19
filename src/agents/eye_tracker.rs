//! Eye tracker agent.

use crate::{
    agents::{mirror, python},
    fisheye::{self, Fisheye},
    pid::{derivative::LowPassFilter, InstantTimer, Timer},
    port,
    port::Port,
};
use eyre::{Error, Result, WrapErr};
use futures::prelude::*;
use std::f64::consts::PI;
use tokio::runtime;

const DISTANCE_RGB_CAMERA_TO_MIRROR_HORIZONTAL: f64 = 8.2;
const DISTANCE_RGB_CAMERA_TO_MIRROR_BACK: f64 = 27.4;
const DISTANCE_RGB_CAMERA_TO_MIRROR_UP: f64 = 44.394;
const RGB_CAMERA_VIEW_ANGLE_HORIZONTAL: f64 = 73.568;
const RGB_CAMERA_VIEW_ANGLE_VERTICAL: f64 = 94.382;
const LOW_PASS_FILTER_RC: f64 = 0.14;

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

impl super::Agent for Agent {
    const NAME: &'static str = "eye-tracker";
}

impl super::AgentThread for Agent {
    fn run(mut self, mut port: port::Inner<Self>) -> Result<()> {
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
                    let dt = timer.get_dt().unwrap_or(0.0);
                    self.calculate_mirror_point(
                        target_left_eye,
                        filter_left_x.add(f64::from(left_x), dt, LOW_PASS_FILTER_RC),
                        filter_left_y.add(f64::from(left_y), dt, LOW_PASS_FILTER_RC),
                        filter_right_x.add(f64::from(right_x), dt, LOW_PASS_FILTER_RC),
                        filter_right_y.add(f64::from(right_y), dt, LOW_PASS_FILTER_RC),
                        user_distance,
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

impl Agent {
    fn calculate_mirror_point(
        &self,
        target_left_eye: bool,
        left_x: f64,
        left_y: f64,
        right_x: f64,
        right_y: f64,
        user_distance: f64,
    ) -> mirror::Point {
        let horizontal_left_eye_percentage_position = left_x;
        let vertical_left_eye_percentage_position = left_y;
        let left_horizontal = calculate_mirror_angle(
            user_distance,
            horizontal_left_eye_percentage_position,
            RGB_CAMERA_VIEW_ANGLE_HORIZONTAL / self.horizontal_multiplier,
            DISTANCE_RGB_CAMERA_TO_MIRROR_BACK,
            DISTANCE_RGB_CAMERA_TO_MIRROR_HORIZONTAL,
            false,
        );
        let left_vertical = -calculate_mirror_angle(
            user_distance,
            vertical_left_eye_percentage_position,
            RGB_CAMERA_VIEW_ANGLE_VERTICAL / self.vertical_multiplier,
            DISTANCE_RGB_CAMERA_TO_MIRROR_BACK,
            DISTANCE_RGB_CAMERA_TO_MIRROR_UP,
            true,
        );

        let horizontal_right_eye_percentage_position = right_x;
        let vertical_right_eye_percentage_position = right_y;
        let right_horizontal = calculate_mirror_angle(
            user_distance,
            horizontal_right_eye_percentage_position,
            RGB_CAMERA_VIEW_ANGLE_HORIZONTAL / self.horizontal_multiplier,
            DISTANCE_RGB_CAMERA_TO_MIRROR_BACK,
            DISTANCE_RGB_CAMERA_TO_MIRROR_HORIZONTAL,
            false,
        );
        let right_vertical = -calculate_mirror_angle(
            user_distance,
            vertical_right_eye_percentage_position,
            RGB_CAMERA_VIEW_ANGLE_VERTICAL / self.vertical_multiplier,
            DISTANCE_RGB_CAMERA_TO_MIRROR_BACK,
            DISTANCE_RGB_CAMERA_TO_MIRROR_UP,
            true,
        );

        if target_left_eye {
            mirror::Point { horizontal: left_horizontal, vertical: left_vertical }
        } else {
            mirror::Point { horizontal: right_horizontal, vertical: right_vertical }
        }
    }
}

fn calculate_mirror_angle(
    user_distance: f64,
    eye_percentage_position: f64,
    camera_view_angle: f64,
    distance_camera_to_mirror_vertical: f64,
    distance_camera_to_mirror_horizontal: f64,
    is_redirect_vertical: bool,
) -> f64 {
    let camera_to_eye_angle =
        (180.0 - camera_view_angle) / 2.0 + eye_percentage_position * camera_view_angle;
    let distance_camera_to_perpendicular_intersect_at_camera_level =
        user_distance / ((180.0 - camera_to_eye_angle) / 180.0 * PI).tan();
    let mirror_to_eye_internal_angle = ((user_distance + distance_camera_to_mirror_vertical)
        / (distance_camera_to_mirror_horizontal
            + distance_camera_to_perpendicular_intersect_at_camera_level))
        .atan()
        / PI
        * 180.0;
    let mirror_to_eye_angle = 180.0 - (180.0 + (mirror_to_eye_internal_angle % 180.0)) % 180.0;
    let mirror_angle = mirror_to_eye_angle / 2.0;
    if is_redirect_vertical { mirror_angle - 45.0 } else { mirror_angle }
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
