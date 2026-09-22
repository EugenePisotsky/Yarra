//! Shared numeric and angular authoring controls.

use bevy_egui::egui;

pub(super) fn drag_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut changed = false;
    egui::Grid::new(("vegetation_f32", label))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(label);
            changed = ui
                .add(egui::DragValue::new(value).speed(speed).range(range))
                .changed();
            ui.end_row();
        });
    changed
}

pub(super) fn drag_u16(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u16,
    range: std::ops::RangeInclusive<u16>,
) -> bool {
    let mut changed = false;
    egui::Grid::new(("vegetation_u16", label))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(label);
            changed = ui
                .add(egui::DragValue::new(value).speed(1).range(range))
                .changed();
            ui.end_row();
        });
    changed
}

pub(super) fn drag_angle_degrees(
    ui: &mut egui::Ui,
    label: &str,
    radians: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut degrees = radians.to_degrees();
    let changed = drag_f32(ui, label, &mut degrees, 0.25, range);
    if changed {
        *radians = degrees.to_radians();
    }
    changed
}
