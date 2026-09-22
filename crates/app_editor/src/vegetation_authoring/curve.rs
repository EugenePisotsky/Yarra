//! Metric ribbon-curve canvas and constrained handle editing.

use bevy::prelude::*;
use bevy_egui::egui;
use vegetation::RibbonCurveProfile;

pub(super) fn draw_ribbon_curve_editor(
    ui: &mut egui::Ui,
    label: &str,
    id_salt: &'static str,
    curve: &mut RibbonCurveProfile,
    tip_tilt_range: std::ops::RangeInclusive<f32>,
    high_section_count: u8,
    paired_main: bool,
    longitudinal_power: f32,
    chart_extent: f32,
) -> bool {
    const EDITOR_HEIGHT: f32 = 230.0;
    const CURVE_SAMPLES: usize = 64;

    let mut changed = false;
    ui.label(label);
    let desired_size = egui::vec2(ui.available_width().max(180.0), EDITOR_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let canvas = CurveCanvas::new(rect.shrink(10.0), chart_extent.clamp(0.6, 3.0));
    let original_controls = ribbon_preview_controls(*curve);

    for control_index in 1..=3 {
        let point = canvas.to_screen(original_controls[control_index]);
        let control_name = match control_index {
            1 => "Root handle",
            2 => "Tip handle",
            _ => "Tip",
        };
        let response = ui
            .interact(
                egui::Rect::from_center_size(point, egui::vec2(22.0, 22.0)),
                ui.id()
                    .with(("ribbon_curve_control", id_salt, control_index)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(control_name);
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            update_ribbon_curve_control(
                curve,
                control_index,
                canvas.from_screen(pointer),
                &tip_tilt_range,
            );
            changed = true;
        }
    }

    let controls = ribbon_preview_controls(*curve);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(52));
    draw_curve_grid(&painter, canvas);

    let unit_arc: Vec<_> = (0..=32)
        .map(|index| {
            let angle = 1.55 * index as f32 / 32.0;
            canvas.to_screen([angle.sin(), angle.cos()])
        })
        .collect();
    painter.add(egui::Shape::line(
        unit_arc,
        egui::Stroke::new(1.0, egui::Color32::from_gray(64)),
    ));
    painter.line_segment(
        [canvas.to_screen(controls[0]), canvas.to_screen(controls[1])],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 156, 176)),
    );
    painter.line_segment(
        [canvas.to_screen(controls[2]), canvas.to_screen(controls[3])],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(188, 145, 58)),
    );

    let mut sampled = Vec::with_capacity(CURVE_SAMPLES + 1);
    for index in 0..=CURVE_SAMPLES {
        let t = index as f32 / CURVE_SAMPLES as f32;
        sampled.push(cubic_preview_point(controls, t));
    }
    painter.add(egui::Shape::line(
        sampled
            .iter()
            .copied()
            .map(|point| canvas.to_screen(point))
            .collect(),
        egui::Stroke::new(2.5, egui::Color32::from_rgb(116, 210, 126)),
    ));

    let sections = u32::from(high_section_count.max(1));
    for row in 0..=sections {
        let linear_t = if paired_main && sections == 4 {
            [0.0_f32, 0.215, 0.5, 0.70, 1.0][row as usize]
        } else if paired_main {
            let middle = sections.div_ceil(2).max(1);
            if row <= middle {
                0.5 * row as f32 / middle as f32
            } else {
                0.5 + 0.5 * (row - middle) as f32 / (sections - middle) as f32
            }
        } else {
            row as f32 / sections as f32
        };
        let t = linear_t.powf(longitudinal_power.max(0.2));
        painter.circle_filled(
            canvas.to_screen(cubic_preview_point(controls, t)),
            2.5,
            egui::Color32::WHITE,
        );
    }

    let point_colors = [
        egui::Color32::from_gray(110),
        egui::Color32::from_rgb(94, 200, 228),
        egui::Color32::from_rgb(238, 190, 78),
        egui::Color32::WHITE,
    ];
    let point_labels = ["R", "1", "2", "T"];
    for index in 0..4 {
        let screen = canvas.to_screen(controls[index]);
        painter.circle_filled(screen, 6.0, point_colors[index]);
        painter.text(
            screen + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            point_labels[index],
            egui::FontId::monospace(10.0),
            point_colors[index],
        );
    }
    ui.small(format!(
        "1 ({:.3}, {:.3})   2 ({:.3}, {:.3})   T ({:.3}, {:.3})   tip {:.1}°",
        controls[1][0],
        controls[1][1],
        controls[2][0],
        controls[2][1],
        controls[3][0],
        controls[3][1],
        curve.tip_tilt_radians.to_degrees(),
    ));
    changed
}

#[derive(Clone, Copy)]
struct CurveCanvas {
    origin: egui::Pos2,
    pixels_per_unit: f32,
    rect: egui::Rect,
}

impl CurveCanvas {
    fn new(rect: egui::Rect, extent: f32) -> Self {
        let pixels_per_unit = (rect.width() / (extent * 2.0))
            .min(rect.height() / (extent * 1.5))
            .max(1.0);
        let view_height = extent * 1.5 * pixels_per_unit;
        let top = rect.center().y - view_height * 0.5;
        Self {
            origin: egui::pos2(rect.center().x, top + extent * 1.25 * pixels_per_unit),
            pixels_per_unit,
            rect,
        }
    }

    fn to_screen(self, point: [f32; 2]) -> egui::Pos2 {
        egui::pos2(
            self.origin.x + point[0] * self.pixels_per_unit,
            self.origin.y - point[1] * self.pixels_per_unit,
        )
    }

    fn from_screen(self, point: egui::Pos2) -> [f32; 2] {
        [
            (point.x - self.origin.x) / self.pixels_per_unit,
            (self.origin.y - point.y) / self.pixels_per_unit,
        ]
    }
}

fn draw_curve_grid(painter: &egui::Painter, canvas: CurveCanvas) {
    let ground_y = canvas.to_screen([0.0, 0.0]).y;
    painter.line_segment(
        [
            egui::pos2(canvas.rect.left(), ground_y),
            egui::pos2(canvas.rect.right(), ground_y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(72)),
    );
    painter.line_segment(
        [
            egui::pos2(canvas.origin.x, canvas.rect.top()),
            egui::pos2(canvas.origin.x, canvas.rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(52)),
    );
}

fn update_ribbon_curve_control(
    curve: &mut RibbonCurveProfile,
    control_index: usize,
    point: [f32; 2],
    tip_tilt_range: &std::ops::RangeInclusive<f32>,
) {
    match control_index {
        1 => update_polar_handle(
            point,
            -1.55..=1.55,
            &mut curve.root_tangent_radians,
            &mut curve.root_handle_length,
        ),
        2 => {
            let controls = ribbon_preview_controls(*curve);
            update_polar_handle(
                [controls[3][0] - point[0], controls[3][1] - point[1]],
                -1.55..=3.05,
                &mut curve.tip_tangent_radians,
                &mut curve.tip_handle_length,
            );
        }
        3 => {
            if point[0].hypot(point[1]) > 1e-4 {
                curve.tip_tilt_radians = point[0]
                    .atan2(point[1])
                    .clamp(*tip_tilt_range.start(), *tip_tilt_range.end());
            }
        }
        _ => {}
    }
}

fn update_polar_handle(
    vector: [f32; 2],
    angle_range: std::ops::RangeInclusive<f32>,
    angle: &mut f32,
    length: &mut f32,
) {
    let requested_length = vector[0].hypot(vector[1]);
    if requested_length > 1e-4 {
        *angle = vector[0]
            .atan2(vector[1])
            .clamp(*angle_range.start(), *angle_range.end());
    }
    *length = requested_length.clamp(0.02, 1.5);
}

fn ribbon_preview_controls(curve: RibbonCurveProfile) -> [[f32; 2]; 4] {
    let p0 = [0.0, 0.0];
    let p3 = [curve.tip_tilt_radians.sin(), curve.tip_tilt_radians.cos()];
    let p1 = [
        curve.root_tangent_radians.sin() * curve.root_handle_length,
        curve.root_tangent_radians.cos() * curve.root_handle_length,
    ];
    let p2 = [
        p3[0] - curve.tip_tangent_radians.sin() * curve.tip_handle_length,
        p3[1] - curve.tip_tangent_radians.cos() * curve.tip_handle_length,
    ];
    [p0, p1, p2, p3]
}

fn cubic_preview_point(points: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    [
        points
            .iter()
            .zip(weights)
            .map(|(point, weight)| point[0] * weight)
            .sum(),
        points
            .iter()
            .zip(weights)
            .map(|(point, weight)| point[1] * weight)
            .sum(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use vegetation::RibbonCurveProfile;

    #[test]
    fn curve_editor_updates_root_and_tip_handles_in_normalized_space() {
        let mut curve = RibbonCurveProfile {
            tip_tilt_radians: 0.6,
            root_tangent_radians: 0.0,
            tip_tangent_radians: 0.0,
            root_handle_length: 0.3,
            tip_handle_length: 0.2,
        };
        update_ribbon_curve_control(&mut curve, 1, [0.3, 0.4], &(0.0..=1.55));
        assert!((curve.root_handle_length - 0.5).abs() < 1e-5);
        assert!((curve.root_tangent_radians - 0.3_f32.atan2(0.4)).abs() < 1e-5);

        let tip = ribbon_preview_controls(curve)[3];
        update_ribbon_curve_control(&mut curve, 2, [tip[0] - 0.4, tip[1] - 0.3], &(0.0..=1.55));
        assert!((curve.tip_handle_length - 0.5).abs() < 1e-5);
        assert!((curve.tip_tangent_radians - 0.4_f32.atan2(0.3)).abs() < 1e-5);
    }
    #[test]
    fn curve_editor_constrains_tip_to_the_authored_variant_range() {
        let mut curve = RibbonCurveProfile {
            tip_tilt_radians: 0.6,
            root_tangent_radians: 0.0,
            tip_tangent_radians: 0.0,
            root_handle_length: 0.3,
            tip_handle_length: 0.2,
        };
        update_ribbon_curve_control(&mut curve, 3, [1.0, 0.0], &(0.2..=0.9));
        assert!((curve.tip_tilt_radians - 0.9).abs() < 1e-5);
        let controls = ribbon_preview_controls(curve);
        assert!((controls[3][0].hypot(controls[3][1]) - 1.0).abs() < 1e-5);
    }
}
