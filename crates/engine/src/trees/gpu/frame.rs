//! Each frame's view of the trees: the world view's eye and frustum, and the cascades of every
//! light that casts, as the compute pass culls against them; and which representations of
//! each form any batch could draw in each view, so only those are queued.
use super::layout::TreeDrawLayout;
use super::pool::TreePool;
use super::{FORCED_MASK, FORCED_NOTHING, MAX_SHADOW_VIEWS, TreeInstancing};
use bevy::{
    camera::primitives::{CascadesFrusta, Frustum},
    light::CascadeShadowConfig,
    prelude::*,
    render::{extract_resource::ExtractResource, sync_world::MainEntity, view::RetainedViewEntity},
};
use std::sync::Arc;

#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct TreeGpuFrame {
    /// None until a world view exists.
    pub(super) view: Option<FrameView>,
    pub(super) dt: f32,
    pub(super) shadow_lod: u32,
    pub(super) hidden: bool,
}

/// Bands are widened by this share and these metres before a representation is ruled out:
/// beyond the 4% hysteresis, an instance keeps dissolving out of its band for 0.6 s.
const BAND_SHARE: f32 = 0.1;
const BAND_METRES: f32 = 20.0;

#[derive(Clone)]
pub(super) struct FrameView {
    pub(super) eye: Vec3,
    pub(super) planes: [Vec4; 6],
    /// Shadow views, in the order the compute pass numbers them.
    pub(super) cascades: Arc<Vec<ShadowView>>,
    /// Per form, a bit for each representation some batch could draw in the main view.
    pub(super) drawn: Arc<Vec<u8>>,
    /// Per shadow view and form, a bit for each representation some batch could cast from.
    pub(super) cast: Arc<Vec<Vec<u8>>>,
}

/// One cascade of a light that casts.
#[derive(Clone)]
pub(super) struct ShadowView {
    pub(super) retained: RetainedViewEntity,
    /// Its frustum's half-spaces.
    pub(super) planes: [Vec4; 6],
    /// The main view's depths that sample it, as near and far half-spaces, and the
    /// direction its light travels: a caster whose shadow cannot reach them is not drawn.
    pub(super) receivers: [Vec4; 3],
}

/// The main view's depths whose fragments sample cascade `index` of `config`: from where
/// the cascade before starts blending into it (Bevy's `fetch_directional_shadow`) to its
/// far bound, as near and far half-spaces.
fn receiver_depths(
    config: &CascadeShadowConfig,
    index: usize,
    eye: Vec3,
    forward: Vec3,
) -> [Vec4; 2] {
    let near = match index {
        0 => 0.0,
        _ => (1.0 - config.overlap_proportion) * config.bounds[index - 1],
    };
    let far = config.bounds.get(index).copied().unwrap_or(f32::MAX);
    let depth = forward.dot(eye);
    [
        forward.extend(-depth - near),
        (-forward).extend(depth + far),
    ]
}

/// Whether a box is on the inner side of every half-space but `skip`.
fn box_in(planes: &[Vec4; 6], lo: Vec3, hi: Vec3, skip: usize) -> bool {
    planes.iter().enumerate().all(|(i, plane)| {
        let normal = plane.truncate();
        let far_corner = Vec3::select(normal.cmpgt(Vec3::ZERO), hi, lo);
        i == skip || normal.dot(far_corner) + plane.w > 0.0
    })
}

/// The representations each form's batches could draw in the main view and cast from in each
/// shadow view, judged per batch from its roots' distance range against each band.
fn representations(
    pool: &TreePool,
    layout: &super::layout::Layout,
    eye: Vec3,
    planes: &[Vec4; 6],
    cascades: &[ShadowView],
    shadow_lod: u32,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let forms = &layout.forms;
    let mut drawn = vec![0u8; forms.len()];
    let mut cast = vec![vec![0u8; forms.len()]; cascades.len()];
    for batch in pool.batches() {
        let Some(form) = forms.get(batch.form as usize) else {
            continue;
        };
        if form.ready == 0 || batch.position[3] == 0.0 {
            continue;
        }
        let origin = Vec3::from_slice(&batch.position);
        let (lo, hi) = (origin + batch.roots.0, origin + batch.roots.1);
        let (smin, smax) = batch.scales;
        // Every representation's bounds round the root, at the largest scale.
        let reach = (0..form.representations as usize)
            .flat_map(|r| {
                let first = form.first_bucket[r / 4][r % 4] as usize;
                first..first + form.buckets[r / 4][r % 4] as usize
            })
            .filter_map(|b| layout.buckets.get(b))
            .map(|b| Vec3::from_slice(&b.centre).length() + Vec3::from_slice(&b.extent).length())
            .fold(0.0, f32::max)
            * smax;
        let (outer_lo, outer_hi) = (lo - reach, hi + reach);
        let nearest = eye.distance(eye.clamp(lo, hi));
        let farthest = eye.distance(Vec3::select((eye - lo).cmpgt(hi - eye), lo, hi));
        let mut representations = 0u8;
        match batch.flags & FORCED_MASK {
            0 => {
                for r in 0..form.representations as usize {
                    let (start, end) = (form.band_start[r / 4][r % 4], form.band_end[r / 4][r % 4]);
                    if start < 0.0 {
                        continue;
                    }
                    let near = start * smin * (1.0 - BAND_SHARE) - BAND_METRES;
                    let far = end * smax * (1.0 + BAND_SHARE) + BAND_METRES;
                    if near <= farthest && far >= nearest {
                        representations |= 1 << r;
                    }
                }
            }
            FORCED_NOTHING => {}
            forced => representations = 1 << (forced - 1),
        }
        if representations == 0 {
            continue;
        }
        if box_in(
            planes,
            outer_lo,
            outer_hi,
            bevy::shape::ViewFrustum::FAR_PLANE_IDX,
        ) {
            drawn[batch.form as usize] |= representations;
        }
        let meshes = form.meshes as usize;
        let mut casters = 0u8;
        for r in (0..meshes).filter(|r| representations & (1 << r) != 0) {
            casters |= 1 << (r + shadow_lod as usize).min(meshes - 1);
        }
        if casters == 0 {
            continue;
        }
        for (view, cascade) in cascades.iter().enumerate() {
            if box_in(
                &cascade.planes,
                outer_lo,
                outer_hi,
                bevy::shape::ViewFrustum::NEAR_PLANE_IDX,
            ) {
                cast[view][batch.form as usize] |= casters;
            }
        }
    }
    (drawn, cast)
}

fn planes(frustum: &Frustum) -> [Vec4; 6] {
    std::array::from_fn(|i| frustum.half_spaces[i].normal_d())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn collect(
    mut frame: ResMut<TreeGpuFrame>,
    settings: Res<TreeInstancing>,
    time: Res<Time>,
    pool: Res<TreePool>,
    layout: Res<TreeDrawLayout>,
    world_view: crate::ActiveWorldView,
    lights: Query<(
        Entity,
        &DirectionalLight,
        &CascadesFrusta,
        &CascadeShadowConfig,
        &GlobalTransform,
        &ViewVisibility,
    )>,
) {
    let view = world_view.active().map(|view| {
        let camera = view.entity;
        let eye = view.transform.translation();
        let forward = view.transform.forward().as_vec3();
        let mut cascades = Vec::new();
        for (light, directional, frusta, config, transform, visible) in &lights {
            if !directional.shadow_maps_enabled || !visible.get() {
                continue;
            }
            for (index, frustum) in frusta.frusta.get(&camera).into_iter().flatten().enumerate() {
                if cascades.len() == MAX_SHADOW_VIEWS {
                    warn_once!("GPU trees: more than {MAX_SHADOW_VIEWS} shadow views; the rest get no tree shadows");
                    break;
                }
                let [near, far] = receiver_depths(config, index, eye, forward);
                cascades.push(ShadowView {
                    retained: RetainedViewEntity::new(
                        MainEntity::from(light),
                        Some(MainEntity::from(camera)),
                        index as u32,
                    ),
                    planes: planes(frustum),
                    receivers: [near, far, transform.forward().extend(0.0)],
                });
            }
        }
        let planes = planes(view.frustum);
        let (drawn, cast) = representations(
            &pool,
            &layout.layout,
            eye,
            &planes,
            &cascades,
            settings.shadow_lod as u32,
        );
        FrameView {
            eye,
            planes,
            cascades: Arc::new(cascades),
            drawn: Arc::new(drawn),
            cast: Arc::new(cast),
        }
    });
    *frame = TreeGpuFrame {
        view,
        dt: time.delta_secs(),
        shadow_lod: settings.shadow_lod as u32,
        hidden: settings.hidden,
    };
}
