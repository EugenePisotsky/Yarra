//! Shared image inspection transform. Magnification never changes the production camera or LOD.
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ComparisonView {
    pub zoom: f32,
    pub center: [f32; 2],
}

impl Default for ComparisonView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: [0.5, 0.5],
        }
    }
}

impl ComparisonView {
    pub fn validate(&self) -> Result<(), String> {
        let half = 0.5 / self.zoom;
        if !self.zoom.is_finite()
            || !(1.0..=6.0).contains(&self.zoom)
            || self
                .center
                .iter()
                .any(|v| !v.is_finite() || *v < half - 0.00001 || *v > 1.0 - half + 0.00001)
        {
            return Err("Invalid linked comparison zoom/pan".into());
        }
        Ok(())
    }

    pub fn zoom_at(&mut self, zoom: f32, anchor: egui::Vec2) {
        let old = self.zoom;
        self.zoom = zoom.clamp(1.0, 6.0);
        for (axis, coordinate) in [anchor.x, anchor.y].into_iter().enumerate() {
            self.center[axis] += (coordinate - 0.5) * (1.0 / old - 1.0 / self.zoom);
        }
        self.clamp_center();
    }

    pub fn pan(&mut self, delta: egui::Vec2, size: egui::Vec2) {
        self.center[0] -= delta.x / size.x.max(1.0) / self.zoom;
        self.center[1] -= delta.y / size.y.max(1.0) / self.zoom;
        self.clamp_center();
    }

    fn clamp_center(&mut self) {
        let half = 0.5 / self.zoom;
        for coordinate in &mut self.center {
            *coordinate = coordinate.clamp(half, 1.0 - half);
        }
    }

    /// Fill equal canvases without stretching. Nonmatching source aspects are center-cropped.
    pub fn uv(&self, source: egui::Vec2, canvas: egui::Vec2) -> egui::Rect {
        let source_aspect = source.x / source.y;
        let canvas_aspect = canvas.x / canvas.y;
        let crop = if source_aspect > canvas_aspect {
            egui::vec2(canvas_aspect / source_aspect, 1.0)
        } else {
            egui::vec2(1.0, source_aspect / canvas_aspect)
        };
        let center = egui::pos2(
            0.5 + (self.center[0] - 0.5) * crop.x,
            0.5 + (self.center[1] - 0.5) * crop.y,
        );
        egui::Rect::from_center_size(center, crop / self.zoom)
    }
}

/// Both image rectangles share exact dimensions, even when the window is short or narrow.
pub(super) fn image_rects(area: egui::Rect, aspect: f32, pair: bool) -> Vec<egui::Rect> {
    let gap = if pair { 16.0 } else { 0.0 };
    let count = if pair { 2.0 } else { 1.0 };
    let width = ((area.width() - gap).max(2.0) / count).min(area.height().max(1.0) * aspect);
    let size = egui::vec2(width, width / aspect);
    let start = area.center() - egui::vec2((width * count + gap) * 0.5, size.y * 0.5);
    (0..count as usize)
        .map(|i| egui::Rect::from_min_size(start + egui::vec2(i as f32 * (width + gap), 0.0), size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linked_zoom_keeps_pointer_anchor_and_pan_bounded() {
        let mut view = ComparisonView::default();
        view.zoom_at(2.0, egui::vec2(0.75, 0.25));
        let uv = view.uv(egui::vec2(1600.0, 900.0), egui::vec2(640.0, 360.0));
        assert_eq!(
            uv.min + uv.size() * egui::vec2(0.75, 0.25),
            egui::pos2(0.75, 0.25)
        );
        view.pan(egui::vec2(10000.0, -10000.0), egui::vec2(640.0, 360.0));
        view.validate().unwrap();
        view.zoom_at(1.0, egui::Vec2::splat(0.5));
        assert_eq!(view, ComparisonView::default());
    }
    #[test]
    fn different_source_resolutions_get_the_same_normalized_crop() {
        let view = ComparisonView {
            zoom: 3.0,
            center: [0.4, 0.6],
        };
        let canvas = egui::vec2(640.0, 360.0);
        assert_eq!(
            view.uv(egui::vec2(1600.0, 900.0), canvas),
            view.uv(egui::vec2(1280.0, 720.0), canvas)
        );
    }
    #[test]
    fn equal_canvases_fit_wide_and_narrow_windows() {
        for size in [egui::vec2(1400.0, 700.0), egui::vec2(700.0, 300.0)] {
            let area = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let rects = image_rects(area, 16.0 / 9.0, true);
            assert_eq!(rects[0].size(), rects[1].size());
            assert!(area.contains_rect(rects[0]) && area.contains_rect(rects[1]));
        }
    }
}
