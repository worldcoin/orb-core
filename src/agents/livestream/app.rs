use crate::{
    agents::{mirror, python, qr_code},
    consts::{AUTOFOCUS_MAX, AUTOFOCUS_MIN, IRIS_SCORE_MIN, IR_LED_MAX_DURATION},
    utils::RkyvNdarray,
};
use egui::{
    Align2, Button, Color32, Context, FontId, Label, Painter, Pos2, Rect, Response, Rounding,
    Stroke, Ui, Vec2,
};
use ndarray::prelude::*;
use std::collections::VecDeque;

const MIRROR_POINT_HISTORY_LEN: usize = 31;

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub struct App {
    phase: Option<&'static str>,
    expanded: Expanded,
    show_mirror_window: bool,
    ir_eye_state: bool,
    ir_face_state: bool,
    rgb_state: bool,
    thermal_state: bool,
    depth_state: bool,
    rgb_net_estimate: Option<python::rgb_net::EstimateOutput>,
    ir_net_estimate: Option<python::ir_net::EstimateOutput>,
    ir_focus: Option<i16>,
    ir_exposure: Option<u16>,
    mirror_points: VecDeque<mirror::Point>,
    qr_code_points: qr_code::Points,
    target_left_eye: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Expanded {
    #[default]
    None,
    Rgb,
    IrEye,
    IrFace,
    Thermal,
    Depth,
}

impl App {
    pub fn expanded(&self) -> Expanded {
        self.expanded
    }

    pub fn clear(&mut self) {
        self.rgb_net_estimate = None;
        self.ir_net_estimate = None;
        self.ir_focus = None;
        self.ir_exposure = None;
        self.mirror_points = VecDeque::new();
        self.qr_code_points = Vec::new();
        self.target_left_eye = false;
    }

    pub fn set_phase(&mut self, name: &'static str) {
        self.phase = Some(name);
        self.qr_code_points = Vec::new();
    }

    pub fn set_rgb_net_estimate(&mut self, rgb_net_estimate: python::rgb_net::EstimateOutput) {
        self.rgb_net_estimate = Some(rgb_net_estimate);
    }

    pub fn set_ir_eye_state(&mut self, ir_eye_state: bool) {
        self.ir_eye_state = ir_eye_state;
    }

    pub fn set_ir_face_state(&mut self, ir_face_state: bool) {
        self.ir_face_state = ir_face_state;
    }

    pub fn set_rgb_state(&mut self, rgb_state: bool) {
        self.rgb_state = rgb_state;
    }

    pub fn set_thermal_state(&mut self, thermal_state: bool) {
        self.thermal_state = thermal_state;
    }

    pub fn set_depth_state(&mut self, depth_state: bool) {
        self.depth_state = depth_state;
    }

    pub fn set_ir_net_estimate(&mut self, ir_net_estimate: python::ir_net::EstimateOutput) {
        self.ir_net_estimate = Some(ir_net_estimate);
    }

    pub fn set_ir_focus(&mut self, ir_focus: i16) {
        self.ir_focus = Some(ir_focus);
    }

    pub fn set_ir_exposure(&mut self, ir_exposure: u16) {
        self.ir_exposure = Some(ir_exposure);
    }

    pub fn set_mirror_point(&mut self, point: mirror::Point) {
        self.mirror_points.push_front(point - mirror::Point::neutral());
        if self.mirror_points.len() > MIRROR_POINT_HISTORY_LEN {
            self.mirror_points.pop_back();
        }
    }

    pub fn set_qr_code_points(&mut self, points: qr_code::Points) {
        self.qr_code_points = points;
    }

    pub fn set_target_left_eye(&mut self, target_left_eye: bool) {
        self.target_left_eye = target_left_eye;
    }

    pub fn update_rgb_viewport(&mut self, ui: &mut Ui, rect: Rect, response: &Response) {
        self.update_rgb_net_estimate(ui, rect);
        self.update_qr_code(ui, rect);
        self.put_expanded_button(ui, rect, response, Expanded::Rgb);
        self.put_capturing_state(ui, rect, self.rgb_state);
    }

    pub fn update_ir_eye_viewport(&mut self, ui: &mut Ui, rect: Rect, response: &Response) {
        self.update_ir_net_estimate(ui, rect);
        self.update_ir_params(ui, rect);
        self.put_expanded_button(ui, rect, response, Expanded::IrEye);
        self.put_capturing_state(ui, rect, self.ir_eye_state);
    }

    pub fn update_ir_face_viewport(&mut self, ui: &mut Ui, rect: Rect, response: &Response) {
        self.put_expanded_button(ui, rect, response, Expanded::IrFace);
        self.put_capturing_state(ui, rect, self.ir_face_state);
    }

    pub fn update_thermal_viewport(&mut self, ui: &mut Ui, rect: Rect, response: &Response) {
        self.put_expanded_button(ui, rect, response, Expanded::Thermal);
        self.put_capturing_state(ui, rect, self.thermal_state);
    }

    pub fn update_depth_viewport(&mut self, ui: &mut Ui, rect: Rect, response: &Response) {
        self.put_expanded_button(ui, rect, response, Expanded::Depth);
        self.put_capturing_state(ui, rect, self.depth_state);
    }

    pub fn update_dashboard(&mut self, ui: &mut Ui, rect: Rect) {
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.label(format!("PHASE: {}", self.phase.unwrap_or("Initializing")));
            if ui
                .button(egui::RichText::new(egui_phosphor::regular::ARROWS_OUT_CARDINAL).size(24.0))
                .clicked()
            {
                self.show_mirror_window = !self.show_mirror_window;
            }
        });
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn update_context(&mut self, ctx: &Context) {
        const SIZE: Vec2 = Vec2::new(300.0, 300.0);
        const CIRCLE_RADIUS: f32 = 5.0;
        if self.show_mirror_window {
            egui::Window::new("Mirror").open(&mut self.show_mirror_window).fixed_size(SIZE).show(
                ctx,
                |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_height(ui.available_height());
                    let paint_point = |point: mirror::Point, alpha: u8| {
                        let x = point.phi_degrees as f32 / 13.0;
                        let y = point.theta_degrees as f32 / 20.0;
                        let center = Pos2::new(
                            ui.clip_rect().min.x + SIZE.x / 2.0 + SIZE.x / 2.0 * x,
                            ui.clip_rect().min.y + SIZE.y / 2.0 + SIZE.y / 2.0 * y,
                        );
                        ui.painter().circle_filled(
                            center,
                            CIRCLE_RADIUS,
                            Color32::from_white_alpha(alpha),
                        );
                    };
                    for (i, point) in self.mirror_points.iter().enumerate() {
                        let alpha = if i == 0 {
                            u8::MAX
                        } else {
                            ((MIRROR_POINT_HISTORY_LEN + 2 - i)
                                * (usize::from(u8::MAX / 4) / (MIRROR_POINT_HISTORY_LEN + 1))
                                - 1) as u8
                        };
                        paint_point(*point, alpha);
                    }
                },
            );
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn update_ir_params(&self, ui: &mut Ui, rect: Rect) {
        if let Some(ir_focus) = self.ir_focus {
            text_with_shadow(
                ui.painter(),
                Pos2::new(rect.min.x + 2.0, rect.min.y + 2.0),
                format!("focus:     {ir_focus}"),
            );
            bar_with_shadow(
                ui.painter(),
                rect,
                26.0,
                f32::from(ir_focus - AUTOFOCUS_MIN) / f32::from(AUTOFOCUS_MAX - AUTOFOCUS_MIN),
            );
        }
        if let Some(ir_exposure) = self.ir_exposure {
            text_with_shadow(
                ui.painter(),
                Pos2::new(rect.min.x + 2.0, rect.min.y + 34.0),
                format!("exposure:  {ir_exposure}"),
            );
            bar_with_shadow(
                ui.painter(),
                rect,
                58.0,
                f32::from(ir_exposure) / f32::from(IR_LED_MAX_DURATION),
            );
        }
        if let Some(ir_net_estimate) = &self.ir_net_estimate {
            text_with_shadow(
                ui.painter(),
                Pos2::new(rect.min.x + 2.0, rect.min.y + 66.0),
                format!("sharpness: {:.4}", ir_net_estimate.sharpness),
            );
            bar_with_shadow(
                ui.painter(),
                rect,
                90.0,
                (ir_net_estimate.sharpness / (IRIS_SCORE_MIN * 1.25)).clamp(0.0, 1.0) as f32,
            );
        }
        if let Some(ir_net_estimate) = &self.ir_net_estimate {
            text_with_shadow(
                ui.painter(),
                Pos2::new(rect.min.x + 2.0, rect.min.y + 98.0),
                format!("target:    {}", if self.target_left_eye { "left" } else { "right" }),
            );
            let side = match ir_net_estimate.perceived_side {
                Some(0) => "left",
                Some(1) => "right",
                _ => "unknown",
            };
            text_with_shadow(
                ui.painter(),
                Pos2::new(rect.min.x + 2.0, rect.min.y + 120.0),
                format!("perceived: {side}"),
            );
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn update_rgb_net_estimate(&self, ui: &mut Ui, rect: Rect) {
        const STROKE: Stroke = Stroke { width: 2.0, color: Color32::YELLOW };
        const CIRCLE_RADIUS: f32 = 8.0;

        // Draw the estimated eye landmarks.
        let Some(prediction) =
            self.rgb_net_estimate.as_ref().and_then(python::rgb_net::EstimateOutput::primary)
        else {
            return;
        };
        let left = Pos2::new(
            rect.min.x + rect.size().x * prediction.landmarks.left_eye.x as f32,
            rect.min.y + rect.size().y * prediction.landmarks.left_eye.y as f32,
        );
        let right = Pos2::new(
            rect.min.x + rect.size().x * prediction.landmarks.right_eye.x as f32,
            rect.min.y + rect.size().y * prediction.landmarks.right_eye.y as f32,
        );
        ui.painter().circle_filled(left, CIRCLE_RADIUS, Color32::RED);
        ui.painter().circle_filled(right, CIRCLE_RADIUS, Color32::GREEN);

        // Draw the bounding box of the face.
        let start = Vec2::new(
            prediction.bbox.coordinates.start_x as f32,
            prediction.bbox.coordinates.start_y as f32,
        );
        let end = Vec2::new(
            prediction.bbox.coordinates.end_x as f32,
            prediction.bbox.coordinates.end_y as f32,
        );
        let min = rect.min + rect.size() * start;
        let max = rect.min + rect.size() * end;
        ui.painter().rect_stroke(Rect::from_min_max(min, max), Rounding::ZERO, STROKE);
    }

    fn update_ir_net_estimate(&self, ui: &mut Ui, rect: Rect) {
        const STROKE: Stroke = Stroke { width: 2.0, color: Color32::YELLOW };
        let Some(ir_net_estimate) = &self.ir_net_estimate else {
            return;
        };
        if let Some(landmarks) =
            ir_net_estimate.landmarks.as_ref().map(RkyvNdarray::<_, Ix2>::as_ndarray)
        {
            let mut landmarks = landmarks.axis_iter(Axis(0)).map(|landmark| {
                Pos2::new(
                    rect.min.x + rect.size().x * landmark[0],
                    rect.min.y + rect.size().y * landmark[1],
                )
            });
            for shape_length in [4, 4, 8] {
                let mut first = None;
                let mut last = None;
                for _ in 0..shape_length {
                    let Some(point) = landmarks.next() else { break };
                    if let Some(last) = last {
                        ui.painter().line_segment([last, point], STROKE);
                    }
                    last = Some(point);
                    if first.is_none() {
                        first = Some(point);
                    }
                }
                if let (Some(first), Some(last)) = (first, last) {
                    ui.painter().line_segment([first, last], STROKE);
                }
            }
        }
    }

    fn update_qr_code(&self, ui: &mut Ui, rect: Rect) {
        const STROKE: Stroke = Stroke { width: 2.0, color: Color32::YELLOW };
        if self.qr_code_points.len() < 2 {
            return;
        }
        let make_pos =
            |(x, y)| Pos2::new(rect.min.x + rect.size().x * x, rect.min.y + rect.size().y * y);
        for i in 0..self.qr_code_points.len() {
            let a = self.qr_code_points[i];
            let b = self.qr_code_points[(i + 1) % self.qr_code_points.len()];
            ui.painter().line_segment([make_pos(a), make_pos(b)], STROKE);
        }
    }

    #[allow(clippy::unused_self)]
    fn put_capturing_state(&self, ui: &mut Ui, rect: Rect, state: bool) {
        let icon = if state { egui_phosphor::regular::PLAY } else { egui_phosphor::regular::PAUSE };
        ui.put(
            Rect::from_two_pos(rect.right_top(), rect.right_top() + Vec2::new(-24.0, 24.0)),
            Label::new(egui::RichText::new(icon).size(24.0)),
        );
    }

    fn put_expanded_button(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        response: &Response,
        expanded: Expanded,
    ) {
        if response.contains_pointer() {
            let icon = if self.expanded == expanded {
                egui_phosphor::regular::ARROWS_IN
            } else {
                egui_phosphor::regular::ARROWS_OUT
            };
            let button = ui.put(
                Rect::from_two_pos(rect.right_top(), rect.right_top() + Vec2::new(-72.0, 24.0)),
                Button::new(egui::RichText::new(icon).size(24.0)).frame(false),
            );
            if button.clicked() {
                self.expanded.toggle(expanded);
            }
        }
    }
}

impl Expanded {
    fn toggle(&mut self, expanded: Self) {
        *self = if *self == expanded { Self::None } else { expanded };
    }
}

#[allow(clippy::needless_pass_by_value)]
fn text_with_shadow(painter: &Painter, pos: Pos2, text: impl ToString) {
    let text = text.to_string();
    painter.text(
        Pos2::new(pos.x + 2.0, pos.y + 2.0),
        Align2::LEFT_TOP,
        text.clone(),
        FontId::monospace(20.0),
        Color32::BLACK,
    );
    painter.text(pos, Align2::LEFT_TOP, text, FontId::monospace(20.0), Color32::WHITE);
}

fn bar_with_shadow(painter: &Painter, rect: Rect, y: f32, value: f32) {
    painter.hline(
        rect.min.x + 4.0..=(rect.min.x + 4.0) + (rect.size().x * 0.25),
        rect.min.y + y + 2.0,
        Stroke { width: 8.0, color: Color32::BLACK },
    );
    painter.hline(
        rect.min.x + 2.0..=(rect.min.x + 2.0) + (rect.size().x * 0.25 - 2.0) * value,
        rect.min.y + y,
        Stroke { width: 8.0, color: Color32::WHITE },
    );
}
