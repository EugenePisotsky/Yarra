//! The draw layout: each form's LOD bands under the current projection, and one bucket per
//! draw entry (a mesh LOD's primitive, or the form's impostor), which the entry's indirect
//! draw reads. Rebuilt when a form becomes ready, batches come or go, or the
//! projection changes (window, display, field of view, Object detail).
use super::super::impostor::ImpostorMaterial;
use super::super::material::TreeWindMaterial;
use super::forms::TreeForms;
use super::pool::TreePool;
use super::{DRAWS, GpuBucket, GpuForm, MAX_REPRESENTATIONS};
use crate::object_lod::LodProjection;
use bevy::{prelude::*, render::extract_resource::ExtractResource};
use std::sync::Arc;

/// What one draw entry draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Drawn {
    /// A primitive of a mesh LOD.
    Mesh {
        mesh: AssetId<Mesh>,
        material: AssetId<TreeWindMaterial>,
    },
    /// The impostor's quads.
    Impostor {
        mesh: AssetId<Mesh>,
        material: AssetId<ImpostorMaterial>,
    },
}

/// One indirect draw per view: its bucket's records (the bucket of the same index) through
/// one primitive's pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DrawEntry {
    pub(super) drawn: Drawn,
    pub(super) form: u32,
    pub(super) representation: u32,
}

#[derive(Default, PartialEq)]
pub(super) struct Layout {
    pub(super) forms: Vec<GpuForm>,
    pub(super) entries: Vec<DrawEntry>,
    /// One per entry.
    pub(super) buckets: Vec<GpuBucket>,
    pub(super) records_per_view: u32,
}

#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct TreeDrawLayout {
    pub(super) layout: Arc<Layout>,
}

#[derive(Default, PartialEq)]
pub(super) struct Built {
    forms: u64,
    pool: u64,
    projection: Option<LodProjection>,
}

pub(super) fn update(
    forms: Res<TreeForms>,
    pool: Res<TreePool>,
    projection: Res<LodProjection>,
    mut layout: ResMut<TreeDrawLayout>,
    mut built: Local<Built>,
    mut log_in: Local<u32>,
) {
    *log_in = log_in.saturating_sub(1);

    if *log_in == 0 {
        *log_in = 600;
        debug!(
            "TREE_GPU forms={}/{} pending_lods={} instances={} entries={} records_per_view={} pixels_per_metre={}",
            forms.forms.iter().filter(|f| f.ready).count(),
            forms.forms.len(),
            forms
                .forms
                .iter()
                .map(|f| f.lods.iter().filter(|l| l.is_none()).count())
                .sum::<usize>(),
            pool.counts.iter().sum::<u32>(),
            layout.layout.entries.len(),
            layout.layout.records_per_view,
            projection.pixels_per_metre(),
        );
    }

    if projection.pixels_per_metre() <= 0.0 {
        return;
    }

    let now = Built {
        forms: forms.revision,
        pool: pool.revision,
        projection: Some(*projection),
    };

    if *built == now {
        return;
    }

    *built = now;
    let next = build(&forms, &pool.counts, &projection);

    if *layout.layout != next {
        layout.layout = Arc::new(next);
    }
}

/// Each representation's distance band at unit scale, between the midpoints of the
/// crossfades distance LOD would use (as `object_lod::timed` sets its bands): mesh LODs
/// too close together for a band of their own get none and are skipped.
pub(super) fn bands(
    projection: &LodProjection,
    thresholds: &[f32],
    height: f32,
) -> ([f32; MAX_REPRESENTATIONS], [f32; MAX_REPRESENTATIONS]) {
    let mut start = [-1.0; MAX_REPRESENTATIONS];
    let mut end = [-1.0; MAX_REPRESENTATIONS];

    for index in 0..thresholds.len().min(MAX_REPRESENTATIONS) {
        let range = projection.range(thresholds, height, index, f32::INFINITY);

        if range.end_margin.end <= 0.0 {
            continue;
        }

        start[index] = (range.start_margin.start + range.start_margin.end) * 0.5;
        end[index] = if range.end_margin.end >= f32::MAX {
            f32::MAX
        } else {
            (range.end_margin.start + range.end_margin.end) * 0.5
        };
    }

    (start, end)
}

pub(super) fn build(forms: &TreeForms, counts: &[u32], projection: &LodProjection) -> Layout {
    let mut layout = Layout::default();

    for (index, form) in forms.forms.iter().enumerate() {
        let capacity = counts.get(index).copied().unwrap_or(0);
        let meshes = form.spec.meshes.len();
        let representations = meshes + 1;

        // A primitive per mesh LOD, then the impostor; all or none of the form's draws.
        let draws = form.lods.iter().flatten().flatten().count() + 1;
        let fits = layout.entries.len() + draws <= DRAWS;

        if form.ready && capacity > 0 && !fits {
            warn_once!("GPU trees: more than {DRAWS} draw entries; some forms are not drawn");
        }

        let mut gpu = GpuForm {
            representations: representations as u32,
            meshes: meshes as u32,
            ready: u32::from(form.ready && capacity > 0 && fits && form.impostor.is_some()),
            height: form.spec.height,
            ..default()
        };

        // The impostor takes over where its views have a texel per output pixel.
        let handoff = projection.impostor_threshold(form.spec.height, form.texels_per_metre);
        let thresholds = form.spec.thresholds(handoff);
        let (start, end) = bands(projection, &thresholds, form.spec.height);

        for r in 0..MAX_REPRESENTATIONS {
            gpu.band_start[r / 4][r % 4] = start[r];
            gpu.band_end[r / 4][r % 4] = end[r];
        }

        if gpu.ready == 1 {
            let mut add =
                |layout: &mut Layout, r: usize, drawn: Drawn, centre: Vec4, extent: Vec3| {
                    if gpu.buckets[r / 4][r % 4] == 0 {
                        gpu.first_bucket[r / 4][r % 4] = layout.buckets.len() as u32;
                    }

                    gpu.buckets[r / 4][r % 4] += 1;
                    layout.buckets.push(GpuBucket {
                        centre: centre.to_array(),
                        extent: extent.extend(0.0).to_array(),
                        records: layout.records_per_view,
                        capacity,
                        ..default()
                    });
                    layout.records_per_view += capacity;
                    layout.entries.push(DrawEntry {
                        drawn,
                        form: index as u32,
                        representation: r as u32,
                    });
                };

            for (lod, parts) in form.lods.iter().enumerate() {
                for part in parts.iter().flatten() {
                    let (lo, hi) = part.bounds;
                    add(
                        &mut layout,
                        lod,
                        Drawn::Mesh {
                            mesh: part.mesh.id(),
                            material: part.material.id(),
                        },
                        ((lo + hi) * 0.5).extend(0.0),
                        (hi - lo) * 0.5,
                    );
                }
            }

            if let Some(material) = &form.impostor {
                let sphere = form.impostor_sphere;
                add(
                    &mut layout,
                    meshes,
                    Drawn::Impostor {
                        mesh: forms.quad.id(),
                        material: material.id(),
                    },
                    sphere.truncate().extend(1.0),
                    Vec3::new(sphere.w, 0.0, 0.0),
                );
            }
        }

        layout.forms.push(gpu);
    }

    layout
}
