//! F1's terrain page gizmos: each streamed page's grid and normal near the camera.
use crate::runtime_settings::RuntimeSettings;
use bevy::prelude::*;
use engine::StreamedTerrainSurface;

pub(super) fn enabled(settings: Res<RuntimeSettings>) -> bool {
    settings.page_gizmos
}

pub(super) fn draw(
    terrain_pages: Query<(&Transform, &StreamedTerrainSurface)>,
    cameras: Query<&Transform, With<Camera3d>>,
    mut gizmos: Gizmos,
) {
    let Some(camera) = cameras.iter().next() else {
        return;
    };
    let camera_xz = camera.translation.xz();

    for (transform, page) in &terrain_pages {
        let center_xz = transform.translation.xz();
        if center_xz.distance_squared(camera_xz) > 96.0_f32.powi(2) {
            continue;
        }

        let resolution = usize::from(page.heightfield.resolution);
        let intervals = resolution - 1;
        let color = if (page.key.cell.x ^ page.key.cell.z) & 1 == 0 {
            Color::srgba(0.1, 0.95, 1.0, 0.9)
        } else {
            Color::srgba(1.0, 0.2, 0.85, 0.9)
        };
        let point = |x: usize, z: usize| {
            let u = x as f32 / intervals as f32;
            let v = z as f32 / intervals as f32;
            Vec3::new(
                transform.translation.x + (u - 0.5) * page.cell_size,
                page.heightfield.height_at(x, z) + 0.035,
                transform.translation.z + (v - 0.5) * page.cell_size,
            )
        };

        for index in 0..intervals {
            gizmos.line(point(index, 0), point(index + 1, 0), color);
            gizmos.line(point(index, intervals), point(index + 1, intervals), color);
            gizmos.line(point(0, index), point(0, index + 1), color);
            gizmos.line(point(intervals, index), point(intervals, index + 1), color);
        }

        let sample = page
            .heightfield
            .sample([page.cell_size * 0.5; 2], page.cell_size);
        let root = Vec3::new(
            transform.translation.x,
            sample.height + 0.045,
            transform.translation.z,
        );
        gizmos.line(
            root,
            root + Vec3::from_array(sample.normal) * 0.75,
            Color::srgb(1.0, 0.95, 0.15),
        );
    }
}
