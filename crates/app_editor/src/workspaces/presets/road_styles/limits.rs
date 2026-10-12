//! Limits for one edited value, preserving every other authored value.
use super::*;
use std::ops::RangeInclusive;

/// Output grids no longer limit a style: near terrain detail draws it as authored, and the
/// compiler broadens what its grids cannot resolve (`environment_compile::RoadDetailLimits`).
#[derive(Clone, Copy)]
pub(super) struct GeometryLimits {
    pub corridor: Option<f32>,
    pub height_step: f32,
}
impl GeometryLimits {
    pub fn in_project(
        p: &CartTrackProfile,
        dense: &SourceWorkingSets,
        project: &ProjectEditorStore,
        preview_space: Option<WorldSpaceId>,
    ) -> Result<Self, String> {
        let mut height_step = 0.25_f32;
        for space in dependent_spaces(p, &dense.roads, preview_space) {
            height_step = height_step.max(
                project
                    .terrain_height_step(space)
                    .ok_or("Terrain height grid is loading")?,
            );
        }
        Ok(Self {
            corridor: minimum_corridor(p, &dense.roads),
            height_step,
        })
    }

    pub fn range(self, field: GeometryField, p: &CartTrackProfile) -> Option<RangeInclusive<f32>> {
        use GeometryField::*;
        let corridor = self.corridor.unwrap_or(64.0);
        let edges = 2.0 * (p.edge_softness + p.edge_variation + p.track_wander);
        let (mut min, mut max) = match field {
            Spacing => (
                0.1_f32.max(p.track_width.next_up()),
                16.0_f32.min(corridor - p.track_width - edges),
            ),
            TrackWidth => (
                0.05_f32
                    .max(p.edge_softness)
                    .max(p.edge_variation / 0.4)
                    .max(p.track_wander),
                8.0_f32
                    .min(p.track_spacing.next_down())
                    .min(corridor - p.track_spacing - edges),
            ),
            Softness => (
                0.01_f32,
                4.0_f32
                    .min(p.track_width)
                    .min((corridor - p.track_spacing - p.track_width) * 0.5 - p.edge_variation),
            ),
            Variation => (
                0.0,
                4.0_f32.min(p.track_width * 0.4).min(
                    (corridor - p.track_spacing - p.track_width) * 0.5
                        - p.edge_softness
                        - p.track_wander,
                ),
            ),
            Wander => (
                0.0,
                p.track_width.min(
                    (corridor - p.track_spacing - p.track_width) * 0.5
                        - p.edge_softness
                        - p.edge_variation,
                ),
            ),
            EdgePatch | BreakupPatch => (0.25, 64.0),
            WanderLength => (0.5, 64.0),
        };
        // Inverse formulae can round outwards. Check their endpoints using the exact
        // forward constraints, without letting an unrelated invalid field prevent repair.
        let accepts = |v: f32| {
            let mut candidate = p.clone();
            *field.value(&mut candidate) = v;
            let width_fits = candidate.minimum_width() <= corridor;
            match field {
                Spacing => v > p.track_width && width_fits,
                TrackWidth => {
                    v < p.track_spacing
                        && p.edge_softness <= v
                        && p.edge_variation <= v * 0.4
                        && p.track_wander <= v
                        && width_fits
                }
                Softness => v <= p.track_width && width_fits,
                Variation => v <= p.track_width * 0.4 && width_fits,
                Wander => v <= p.track_width && width_fits,
                EdgePatch | BreakupPatch | WanderLength => true,
            }
        };
        for _ in 0..8 {
            if accepts(min) {
                break;
            }
            min = min.next_up();
        }
        for _ in 0..8 {
            if accepts(max) {
                break;
            }
            max = max.next_down();
        }
        // A value squeezed to its last valid setting can leave rounded ends crossed.
        if max < min && accepts(min) {
            max = min;
        }
        (min.is_finite() && max.is_finite() && min <= max && accepts(min) && accepts(max))
            .then_some(min..=max)
    }
}

pub(super) fn minimum_corridor(p: &CartTrackProfile, roads: &RoadWorkingSet) -> Option<f32> {
    let saved = roads
        .style_usage
        .get(&p.id)
        .into_iter()
        .flatten()
        .filter_map(|u| u.minimum_width);
    let routes = roads
        .entries
        .values()
        .filter_map(|e| match &e.current {
            Some(RoadSourceRecord::Road(r)) if r.road.profile == p.id => Some(r.road.id),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let edited = roads.entries.values().filter_map(|e| match &e.current {
        Some(RoadSourceRecord::Knot(k)) if routes.contains(&k.road) => Some(k.knot.width),
        _ => None,
    });
    let retained = routes
        .iter()
        .filter_map(|id| roads.route_minimum_widths.get(id).copied().flatten());
    saved.chain(edited).chain(retained).reduce(f32::min)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum GeometryField {
    Spacing,
    TrackWidth,
    Softness,
    Variation,
    EdgePatch,
    BreakupPatch,
    Wander,
    WanderLength,
}
impl GeometryField {
    pub fn value(self, p: &mut CartTrackProfile) -> &mut f32 {
        match self {
            Self::Spacing => &mut p.track_spacing,
            Self::TrackWidth => &mut p.track_width,
            Self::Softness => &mut p.edge_softness,
            Self::Variation => &mut p.edge_variation,
            Self::EdgePatch => &mut p.edge_patch_size,
            Self::BreakupPatch => &mut p.breakup_patch_size,
            Self::Wander => &mut p.track_wander,
            Self::WanderLength => &mut p.track_wander_length,
        }
    }
    pub fn control(self, ui: &mut egui::Ui, p: &mut CartTrackProfile, limits: GeometryLimits) {
        let label = match self {
            Self::Spacing => "Wheel spacing",
            Self::TrackWidth => "Track width",
            Self::Softness => "Edge softness",
            Self::Variation => "Edge variation",
            Self::EdgePatch => "Edge patch size",
            Self::BreakupPatch => "Breakup patch size",
            Self::Wander => "Track wander",
            Self::WanderLength => "Track wander length",
        };
        if let Some(range) = limits.range(self, p) {
            let hint = format!(
                "Allowed: {:.4}–{:.4} m with the current track dimensions and road widths.",
                range.start(),
                range.end()
            );
            ui.horizontal(|ui| {
                bounded_metres(ui, self.value(p), range, 0.01).on_hover_text(&hint);
                ui.label(label).on_hover_text(hint);
            });
        } else {
            ui.label(format!(
                "{label}: adjust the other track dimensions or widen the road first."
            ));
        }
    }
}

pub(in crate::workspaces::presets) fn bounded_metres(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    speed: f64,
) -> egui::Response {
    let before = *value;
    let response = ui.add(
        egui::DragValue::new(&mut *value)
            .range(range.clone())
            .clamp_existing_to_range(false)
            .max_decimals(4)
            .speed(speed)
            .suffix(" m"),
    );
    // DragValue clamps text and pointer edits, but keyboard increments can exceed
    // its range. Clamp only explicit edits; merely showing a draft must not edit it.
    if response.changed() {
        *value = if value.is_finite() {
            value.clamp(*range.start(), *range.end())
        } else {
            before
        };
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::presets::tests::road_request;

    const FIELDS: [GeometryField; 8] = [
        GeometryField::Spacing,
        GeometryField::TrackWidth,
        GeometryField::Softness,
        GeometryField::Variation,
        GeometryField::EdgePatch,
        GeometryField::BreakupPatch,
        GeometryField::Wander,
        GeometryField::WanderLength,
    ];
    fn limits() -> GeometryLimits {
        GeometryLimits {
            corridor: Some(3.5),
            height_step: 0.25,
        }
    }

    #[test]
    fn reported_softness_error_has_an_explanation_and_a_valid_recovery_range() {
        let mut r = road_request();
        let p = &mut r.road.as_mut().unwrap().profile;
        p.edge_softness = 1.87;
        assert!(
            p.validate()
                .unwrap_err()
                .to_string()
                .contains("edge softness must not exceed track width")
        );
        let allowed = limits().range(GeometryField::Softness, p).unwrap();
        assert!((allowed.end() - 0.57).abs() < 0.00001);
        assert!(!allowed.contains(&1.87));
        p.edge_softness = *allowed.end();
        r.compile().unwrap();
    }

    #[test]
    fn every_control_endpoint_compiles_in_the_production_preview() {
        for size in [8, 16] {
            for curved in [false, true] {
                for field in FIELDS {
                    for upper in [false, true] {
                        let mut r = road_request();
                        r.size = size;
                        let road = r.road.as_mut().unwrap();
                        road.curved = curved;
                        let range = limits().range(field, &road.profile).unwrap();
                        *field.value(&mut road.profile) =
                            if upper { *range.end() } else { *range.start() };
                        r.compile().unwrap_or_else(|e| {
                            panic!("{field:?}, upper={upper}, size={size}, curved={curved}: {e}")
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn repeated_extreme_edits_preserve_other_values_and_all_geometry_constraints() {
        let mut p = road_request().road.unwrap().profile;
        let l = limits();
        for iteration in 0..100 {
            for field in FIELDS {
                let range = l
                    .range(field, &p)
                    .unwrap_or_else(|| panic!("{field:?}: {p:?}"));
                *field.value(&mut p) = if iteration % 2 == 0 {
                    *range.start()
                } else {
                    *range.end()
                };
                p.validate().unwrap();
                assert!(p.minimum_width() <= 3.5);
            }
        }
    }

    #[test]
    fn rendering_controls_does_not_silently_repair_an_invalid_draft() {
        let mut p = road_request().road.unwrap().profile;
        p.edge_softness = 1.87;
        let before = p.clone();
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for field in FIELDS {
                field.control(ui, &mut p, limits());
            }
        })
        // Headless: no renderer consumes the font atlas upload.
        .textures_delta
        .clear();
        assert_eq!(p, before);
    }

    #[test]
    fn numeric_input_clamps_typed_values_and_keyboard_increments() {
        let ctx = egui::Context::default();
        let mut value = 0.15;
        let p = road_request().road.unwrap().profile;
        let range = limits().range(GeometryField::Softness, &p).unwrap();
        let mut id = None;
        let mut frame = |events| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let response = bounded_metres(ui, &mut value, range.clone(), 0.01);
                        id = Some(response.id);
                    });
                },
            )
            // Headless: no renderer consumes the font atlas upload.
            .textures_delta
            .clear();
            (value, id.unwrap())
        };
        let (_, id) = frame(vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(vec![]);
        assert!(
            ctx.memory(|m| m.has_focus(id)),
            "numeric input must have keyboard focus"
        );
        let (typed, _) = frame(vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Text("1.87".into()),
        ]);
        assert_eq!(typed, *range.end());
        let (incremented, _) = frame(vec![egui::Event::Key {
            key: egui::Key::ArrowUp,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(incremented, *range.end());
    }
}
