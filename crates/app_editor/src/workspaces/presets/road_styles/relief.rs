use super::*;
pub(super) fn controls(ui: &mut egui::Ui, p: &mut CartTrackProfile, step: f32) {
    ui.separator();
    ui.heading("Road relief");
    let r = &mut p.relief;
    dimension(ui, "Whole-road depth", &mut r.road_depth, 0.0..=2.0);
    dimension(ui, "Additional track depth", &mut r.track_depth, 0.0..=0.5);
    ui.small("Depths combine: wheel ruts sit below the road bed. Zero depth disables that effect.");
    dimension(
        ui,
        "Terrain shoulder falloff",
        &mut r.shoulder_falloff,
        step.max(0.05)..=8.0,
    );
    ui.add(egui::Slider::new(&mut r.rut_roundness, 0.0..=1.0).text("Rut roundness"));
    ui.small("Roundness 1 gives rounded ruts; 0 gives flatter bottoms. Terrain falloff is independent of material edge softness.");
    ui.collapsing("Depth variation", |ui| {
        percentage(ui,"Road depth variation",&mut r.road_variation);
        dimension(ui,"Road variation length",&mut r.road_variation_length,(step*4.0).max(0.5)..=64.0);
        percentage(ui,"Track depth variation",&mut r.track_variation);
        dimension(ui,"Track variation length",&mut r.track_variation_length,(step*4.0).max(0.5)..=64.0);
        ui.small(format!("Road depth: {:.2}–{:.2} m; additional rut depth: {:.2}–{:.2} m.",r.road_depth*(1.0-r.road_variation),r.road_depth*(1.0+r.road_variation),r.track_depth*(1.0-r.track_variation),r.track_depth*(1.0+r.track_variation)));
        ui.small("Smooth, stable variation uses each road's seed. Both tracks share broad variation but have individual unevenness. Grass patches do not erase ruts.");
    });
    ui.small(format!(
        "Height grid limit: {step:.3} m. Ruts need tracks at least {:.3} m wide.",
        step * 2.0
    ));
    if let Err(e) = p.validate_relief_detail(step) {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
}
fn dimension(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) {
    if range.start() > range.end() {
        ui.label(format!("{label}: terrain grid is too coarse"));
        return;
    }
    ui.horizontal(|ui| {
        bounded_metres(ui, value, range, 0.01);
        ui.label(label);
    });
}
fn percentage(ui: &mut egui::Ui, label: &str, value: &mut f32) {
    let mut percent = *value * 100.0;
    if ui
        .add(
            egui::Slider::new(&mut percent, 0.0..=100.0)
                .suffix("%")
                .text(label),
        )
        .changed()
    {
        *value = percent / 100.0;
    }
}
