//! Snow falling off spruces where characters push through them (`shaders/tree/shed.wesl`).
//! Where a character's body meets the snow a crown holds (its form's [`SnowProfile`]), it shakes
//! the boughs there: the spot and the heights it reaches are logged in world coordinates and drawn
//! into a map wrapping around the player, 64 m across at 25 cm, of the lowest and highest height
//! shaken over each texel and when. Snowy tree vertices read it along their limb, so a bough
//! shaken anywhere sheds beyond that: its snow slides off over a moment and its loads drop, while
//! the snow falls as powder and clumps ([`SnowFalls`]). The map holds nothing the log does not:
//! once the player has gone 16 m it is drawn again from the log around them, so shaken trees stay
//! bare for as long as the log holds them.
use super::gpu::TreeForms;
use super::impostor::ImpostorBatch;

use atmosphere::ambient_particles::{SnowFall, SnowFalls};

use bevy::{
    asset::RenderAssetUsages,
    math::{DVec2, I64Vec2},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
            TextureDimension, TextureFormat,
        },
        renderer::RenderQueue,
        storage::{GpuShaderBuffer, ShaderBuffer},
        texture::GpuImage,
    },
};

use bytemuck::{Pod, Zeroable};
use std::{collections::VecDeque, f32::consts::TAU, sync::Arc};
use vegetation_render::{VegetationBenders, VegetationLodFocus, VegetationRenderOrigin};

/// Texels per side of the map, and metres per texel.
const SIDE: u32 = 256;

const TEXEL_METRES: f64 = 0.25;

/// The map is drawn again around the player once they are this far from its centre, metres.
const RECENTRE_METRES: f64 = 16.0;

/// Shakes the log keeps: a walker through snowy spruces logs about ten a second, so most of an
/// hour of it.
const LOG_CAPACITY: usize = 32768;

/// A body is logged again once it moves this far, metres: its shaken discs overlap closely.
const LOG_MOVED: f64 = 0.1;

/// A body shakes the boughs this far from its axis, from a little below its feet to a little
/// above its head, metres: a bough it pushes shakes along its length, beyond the foliage it
/// parts (touch's body and margin, 0.42 m).
const SHAKE_RADIUS: f32 = 0.75;

const SHAKE_BELOW: f32 = 0.2;

const SHAKE_ABOVE: f32 = 2.2;

/// Snow a body shakes loose falls in a burst once this much air is newly shaken (cubic metres:
/// about 15 cm of a walk), and a whole burst holds this much.
const BURST_VOLUME: f32 = 0.6;

const WHOLE_BURST: f32 = 2.0;

/// How far round where it lay the snow of a burst starts, metres; the share of the way to the
/// trunk it lies ahead of the body, which hides what lies behind it.
const BURST_RADIUS: f32 = 0.5;

const BURST_AHEAD: f32 = 0.3;

/// A profile's height steps up from the root, metres at unit scale, and its bearings.
const PROFILE_STEP: f32 = 0.25;

const SECTORS: usize = 16;

const FORMAT: TextureFormat = TextureFormat::Rgba32Float;

/// A texel nothing shook: its lowest height above its highest.
const UNSHAKEN: [f32; 4] = [f32::MAX, f32::MIN, 0.0, 0.0];

pub(super) fn map_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("5d1c8f3a-7e2b-4a96-b0c4-1f8e3d6a9c27")
}

pub(super) fn window_handle() -> Handle<ShaderBuffer> {
    bevy::asset::uuid_handle!("c83e1a5f-2d9b-4f60-8e71-6a4b0d2c9f35")
}

/// Where a form's snow lies: per height step up from its root and bearing, how far from its
/// axis it reaches, at unit scale; 0 where none lies.
#[derive(Debug, PartialEq)]
pub(crate) struct SnowProfile {
    reach: Vec<[f32; SECTORS]>,
    furthest: f32,
}

impl SnowProfile {
    /// The profile of snow lying at `points` (object space); none without any.
    pub(crate) fn of(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut reach: Vec<[f32; SECTORS]> = Vec::new();

        for p in points.into_iter().filter(|p| p.y >= 0.0) {
            let step = (p.y / PROFILE_STEP) as usize;

            if reach.len() <= step {
                reach.resize(step + 1, [0.0; SECTORS]);
            }

            let slot = &mut reach[step][sector(p.xz())];
            *slot = slot.max(p.xz().length());
        }

        let furthest = reach.iter().flatten().fold(0.0_f32, |a, &b| a.max(b));
        (furthest > 0.0).then_some(Self { reach, furthest })
    }

    /// The lowest and highest heights between `low` and `high` at which the snow reaches a body
    /// of `radius` standing at `at`, in the profile's space.
    fn contact(&self, at: Vec2, radius: f32, low: f32, high: f32) -> Option<(f32, f32)> {
        let distance = at.length();

        if high < 0.0 || distance - radius > self.furthest {
            return None;
        }

        // The bearings the body spans: all of them round the trunk.
        let width = TAU / SECTORS as f32;
        let span = if distance <= radius {
            0..SECTORS as i32
        } else {
            let half = (radius / distance).asin();
            let bearing = at.y.atan2(at.x);
            let first = ((bearing - half) / width).floor() as i32;
            first..((bearing + half) / width).floor() as i32 + 1
        };
        let steps = (low / PROFILE_STEP).floor().max(0.0) as usize
            ..((high / PROFILE_STEP).floor() as usize + 1).min(self.reach.len());
        let mut met: Option<(usize, usize)> = None;

        for step in steps {
            let reaches = span.clone().any(|s| {
                let reach = self.reach[step][s.rem_euclid(SECTORS as i32) as usize];
                reach > 0.0 && reach >= distance - radius
            });

            if reaches {
                met = Some((met.map_or(step, |m| m.0), step));
            }
        }

        met.map(|(first, last)| {
            (
                (first as f32 * PROFILE_STEP).max(low),
                ((last + 1) as f32 * PROFILE_STEP).min(high),
            )
        })
    }
}

fn sector(xz: Vec2) -> usize {
    let turn = (xz.y.atan2(xz.x) / TAU).rem_euclid(1.0);
    ((turn * SECTORS as f32) as usize).min(SECTORS - 1)
}

/// A batch's form's snow once its form is ready; none for forms that hold none.
#[derive(Component)]
pub(super) struct SnowyCrown(Option<Arc<SnowProfile>>);

/// A body shaking boughs: where (canonical x and z) and the heights it reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shake {
    centre: DVec2,
    low: f32,
    high: f32,
    /// On the game clock (`Time`).
    time: f32,
}

/// The map as the CPU keeps it: texels hold fixed world spots, wrapping round the window.
struct ShedMap {
    /// The canonical texel of the window's first corner, once placed.
    first: Option<I64Vec2>,
    /// Per texel: the lowest and highest height shaken and when it was first.
    texels: Vec<[f32; 4]>,
    /// Canonical texel rows changed since the last upload, lowest and highest, or all of them.
    rows: Option<(i64, i64)>,
    whole: bool,
    /// Canonical texels within which every shaken one lies, lowest and highest.
    shaken: Option<(I64Vec2, I64Vec2)>,
}

impl Default for ShedMap {
    fn default() -> Self {
        Self {
            first: None,
            texels: vec![UNSHAKEN; (SIDE * SIDE) as usize],
            rows: None,
            whole: false,
            shaken: None,
        }
    }
}

impl ShedMap {
    /// Keeps the window round canonical `focus`, drawing it again from `log` once it moves.
    fn follow(&mut self, focus: DVec2, log: &VecDeque<Shake>) {
        let half = i64::from(SIDE / 2);
        let centre = self
            .first
            .map(|first| (first + half).as_dvec2() * TEXEL_METRES);

        if centre.is_some_and(|c| c.distance(focus) <= RECENTRE_METRES) {
            return;
        }

        self.first = Some((focus / TEXEL_METRES).floor().as_i64vec2() - half);
        self.texels.fill(UNSHAKEN);
        self.shaken = None;
        self.whole = true;

        for shake in log {
            self.stamp(shake);
        }
    }

    /// Shakes the texels round `shake` within the window; returns the air newly shaken, cubic
    /// metres.
    fn stamp(&mut self, shake: &Shake) -> f32 {
        let Some(first) = self.first else {
            return 0.0;
        };
        let radius = f64::from(SHAKE_RADIUS);
        let last = first + i64::from(SIDE) - 1;
        let low = ((shake.centre - radius) / TEXEL_METRES)
            .floor()
            .as_i64vec2()
            .max(first);
        let high = ((shake.centre + radius) / TEXEL_METRES)
            .floor()
            .as_i64vec2()
            .min(last);
        let mut grown = 0.0;

        for z in low.y..=high.y {
            for x in low.x..=high.x {
                let middle = (I64Vec2::new(x, z).as_dvec2() + 0.5) * TEXEL_METRES;

                if middle.distance_squared(shake.centre) > radius * radius {
                    continue;
                }

                let texel = &mut self.texels[index(x, z)];
                let more = if texel[0] > texel[1] {
                    *texel = [shake.low, shake.high, shake.time, 0.0];
                    shake.high - shake.low
                } else {
                    // Snow it shook before has gone: only the heights added are new.
                    let more = (texel[0] - shake.low).max(0.0) + (shake.high - texel[1]).max(0.0);
                    texel[0] = texel[0].min(shake.low);
                    texel[1] = texel[1].max(shake.high);
                    more
                };

                if more <= 0.0 {
                    continue;
                }

                grown += more;
                let spot = I64Vec2::new(x, z);
                self.rows = Some(self.rows.map_or((z, z), |(a, b)| (a.min(z), b.max(z))));
                self.shaken = Some(
                    self.shaken
                        .map_or((spot, spot), |(a, b)| (a.min(spot), b.max(spot))),
                );
            }
        }

        grown * (TEXEL_METRES * TEXEL_METRES) as f32
    }

    /// The texture rows changed since the last call, as runs of whole rows from a row.
    fn take_rows(&mut self) -> Vec<(u32, Vec<[f32; 4]>)> {
        let side = SIDE as usize;
        let rows = self.rows.take();

        if std::mem::take(&mut self.whole) {
            return vec![(0, self.texels.clone())];
        }

        let Some((low, high)) = rows else {
            return Vec::new();
        };
        let start = low.rem_euclid(i64::from(SIDE)) as usize;
        let end = start + ((high - low + 1) as usize).min(side);

        // The run wraps past the texture's last row at most once.
        [(start, end.min(side)), (0, end.saturating_sub(side))]
            .into_iter()
            .filter(|(a, b)| a < b)
            .map(|(a, b)| (a as u32, self.texels[a * side..b * side].to_vec()))
            .collect()
    }

    fn window(&self, origin: [f64; 2], now: f32) -> ShedWindow {
        let Some(first) = self.first else {
            return ShedWindow::default();
        };
        let corner = first.as_dvec2() * TEXEL_METRES;
        let shaken = self.shaken.map_or([1.0, 1.0, -1.0, -1.0], |(a, b)| {
            let a = a.as_dvec2() * TEXEL_METRES;
            let b = (b + 1).as_dvec2() * TEXEL_METRES;
            [a.x as f32, a.y as f32, b.x as f32, b.y as f32]
        });
        ShedWindow {
            area: [
                corner.x as f32,
                corner.y as f32,
                TEXEL_METRES as f32,
                SIDE as f32,
            ],
            origin: [origin[0] as f32, origin[1] as f32, now, 0.0],
            shaken,
        }
    }
}

/// The texture texel holding canonical texel (`x`, `z`).
fn index(x: i64, z: i64) -> usize {
    let side = i64::from(SIDE);
    (z.rem_euclid(side) * side + x.rem_euclid(side)) as usize
}

/// A body among the benders, by their order: where it was last logged and the air it has newly
/// shaken since its last burst.
#[derive(Clone, Copy, Default)]
struct Body {
    logged: Option<DVec2>,
    shaken: f32,
}

/// Where characters shook snow off trees.
#[derive(Resource, Default)]
pub(super) struct SnowShed {
    log: VecDeque<Shake>,
    map: ShedMap,
    bodies: Vec<Body>,
}

impl SnowShed {
    /// A body shaking boughs: drawn, and logged once it has moved on; returns the air newly
    /// shaken.
    fn shake(&mut self, body: usize, shake: Shake) -> f32 {
        let grown = self.map.stamp(&shake);
        let logged = &mut self.bodies[body].logged;

        if logged.is_none_or(|l| l.distance(shake.centre) >= LOG_MOVED) {
            *logged = Some(shake.centre);

            if self.log.len() == LOG_CAPACITY {
                self.log.pop_front();
            }

            self.log.push_back(shake);
        }

        grown
    }
}

/// Where the map lies (`ShedWindow` in `shaders/tree/shed.wesl`), which every tree material binds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
struct ShedWindow {
    /// The canonical x and z of the window's first texel's corner, metres per texel and texels
    /// per side (0 without a map).
    area: [f32; 4],
    /// The render origin's canonical x and z, and the game clock.
    origin: [f32; 4],
    /// The canonical x and z every shaken texel lies within: lowest, then highest (the lowest
    /// above the highest while none is).
    shaken: [f32; 4],
}

/// This frame's window and the map rows it changed.
#[derive(Resource, ExtractResource, Clone, Default)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct ShedFrame {
    window: ShedWindow,
    rows: Vec<(u32, Vec<[f32; 4]>)>,
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<SnowShed>()
        .init_resource::<ShedFrame>()
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            (find_crowns, shake)
                .chain()
                .after(crate::world_vegetation::list_vegetation_benders)
                .after(bevy::transform::TransformSystems::Propagate),
        );

    if app.get_sub_app(RenderApp).is_none() {
        return;
    }

    app.add_plugins(ExtractResourcePlugin::<ShedFrame>::default());
    app.sub_app_mut(RenderApp)
        .add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
}

/// The map and its window under the handles every tree material binds.
fn setup(mut images: ResMut<Assets<Image>>, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let map = Image::new_fill(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytemuck::bytes_of(&UNSHAKEN),
        FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    images
        .insert(map_handle().id(), map)
        .expect("the shed map's handle is free");
    buffers
        .insert(
            window_handle().id(),
            ShaderBuffer::new(vec![ShedWindow::zeroed()], RenderAssetUsages::RENDER_WORLD),
        )
        .expect("the shed window's handle is free");
}

/// Notes each batch's snow once its form is ready.
fn find_crowns(
    mut commands: Commands,
    forms: Res<TreeForms>,
    batches: Query<(Entity, &ImpostorBatch), Without<SnowyCrown>>,
) {
    for (entity, batch) in &batches {
        if forms.ready(&batch.form.impostor) {
            commands
                .entity(entity)
                .insert(SnowyCrown(forms.snow(&batch.form.impostor)));
        }
    }
}

/// Where the bodies among the plants are: the player the map centres on, the render origin and
/// the characters.
#[derive(bevy::ecs::system::SystemParam)]
struct Bodies<'w> {
    focus: Option<Res<'w, VegetationLodFocus>>,
    origin: Option<Res<'w, VegetationRenderOrigin>>,
    benders: Option<Res<'w, VegetationBenders>>,
}

/// Bodies meeting snowy crowns shake them; the snow they newly shake loose falls in bursts.
fn shake(
    time: Res<Time>,
    bodies: Bodies,
    crowns: Query<(&ImpostorBatch, &GlobalTransform, &SnowyCrown)>,
    mut shed: ResMut<SnowShed>,
    mut falls: Option<ResMut<SnowFalls>>,
    mut frame: ResMut<ShedFrame>,
) {
    let Bodies {
        focus,
        origin,
        benders,
    } = bodies;
    let origin = origin.map_or([0.0; 2], |o| o.world_xz);
    let canonical = |p: Vec3| DVec2::new(f64::from(p.x) + origin[0], f64::from(p.z) + origin[1]);
    let now = time.elapsed_secs();
    let shed = shed.as_mut();

    if let Some(feet) = focus.and_then(|f| f.position) {
        let log = std::mem::take(&mut shed.log);
        shed.map.follow(canonical(feet), &log);
        shed.log = log;
    }

    let benders = benders.as_deref().map_or(&[][..], |b| &b.benders[..]);
    shed.bodies.resize(benders.len(), Body::default());

    for (body, bender) in benders.iter().enumerate() {
        let feet = bender.position;
        let low = feet.y - SHAKE_BELOW;
        let high = feet.y + SHAKE_ABOVE;
        let Some(met) = meet(&crowns, feet, low, high) else {
            continue;
        };
        let shake = Shake {
            centre: canonical(feet),
            low,
            high,
            time: now,
        };
        shed.bodies[body].shaken += shed.shake(body, shake);

        let Some(falls) = falls.as_deref_mut() else {
            continue;
        };

        if shed.bodies[body].shaken < BURST_VOLUME {
            continue;
        }

        let ahead = (met.trunk - feet.xz()).clamp_length_max(BURST_AHEAD);
        let fall = SnowFall {
            position: Vec3::new(feet.x + ahead.x, met.low, feet.z + ahead.y),
            radius: BURST_RADIUS,
            height: met.high - met.low,
            amount: shed.bodies[body].shaken / WHOLE_BURST,
            velocity: bender.velocity,
            ground: feet.y,
            time: now,
        };
        debug!(
            "TREE_SNOW_SHED burst at={:.1} height={:.2} amount={:.2} logged={}",
            fall.position,
            fall.height,
            fall.amount,
            shed.log.len()
        );
        falls.spawn(fall);
        shed.bodies[body].shaken = 0.0;
    }

    *frame = ShedFrame {
        window: shed.map.window(origin, now),
        rows: shed.map.take_rows(),
    };
}

/// Where a body meets snow: the heights over all the crowns it meets, and the nearest trunk.
struct Met {
    low: f32,
    high: f32,
    trunk: Vec2,
}

/// The snow a body standing at render-space `feet` meets between heights `low` and `high`.
fn meet(
    crowns: &Query<(&ImpostorBatch, &GlobalTransform, &SnowyCrown)>,
    feet: Vec3,
    low: f32,
    high: f32,
) -> Option<Met> {
    let mut met: Option<Met> = None;

    for (batch, transform, crown) in crowns {
        let Some(profile) = crown.0.as_deref() else {
            continue;
        };

        for tree in &batch.instances {
            let root = transform.transform_point(tree.translation);
            let offset = feet - root;

            if offset.xz().length() > profile.furthest * tree.scale + SHAKE_RADIUS {
                continue;
            }

            let local = Quat::from_rotation_y(-tree.yaw) * offset / tree.scale;
            let Some((a, b)) = profile.contact(
                local.xz(),
                SHAKE_RADIUS / tree.scale,
                (low - root.y) / tree.scale,
                (high - root.y) / tree.scale,
            ) else {
                continue;
            };
            let (a, b) = (root.y + a * tree.scale, root.y + b * tree.scale);
            let nearer = met
                .as_ref()
                .is_none_or(|m| m.trunk.distance(feet.xz()) > root.xz().distance(feet.xz()));
            met = Some(match met {
                Some(m) => Met {
                    low: m.low.min(a),
                    high: m.high.max(b),
                    trunk: if nearer { root.xz() } else { m.trunk },
                },
                None => Met {
                    low: a,
                    high: b,
                    trunk: root.xz(),
                },
            });
        }
    }

    met
}

/// Writes the window and the map rows the frame changed.
fn upload(
    frame: Res<ShedFrame>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
) {
    if let Some(window) = buffers.get(&window_handle()) {
        queue.write_buffer(&window.buffer, 0, bytemuck::bytes_of(&frame.window));
    }

    let Some(map) = images.get(&map_handle()) else {
        return;
    };

    for (row, texels) in &frame.rows {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &map.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: 0,
                    y: *row,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            bytemuck::cast_slice(texels),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIDE * size_of::<[f32; 4]>() as u32),
                rows_per_image: None,
            },
            Extent3d {
                width: SIDE,
                height: texels.len() as u32 / SIDE,
                depth_or_array_layers: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone of snow: reaching 2 m out at its foot, 1 m up its stem, to nothing at 5 m.
    fn cone() -> SnowProfile {
        let points = (0..200).flat_map(|i| {
            let y = 1.0 + i as f32 * 0.02;
            let r = 2.0 * (5.0 - y) / 4.0;
            (0..32).map(move |k| {
                let a = k as f32 / 32.0 * TAU;
                Vec3::new(a.cos() * r, y, a.sin() * r)
            })
        });
        SnowProfile::of(points).unwrap()
    }

    #[test]
    fn a_body_meets_the_snow_it_reaches_and_only_at_its_heights() {
        let cone = cone();
        let (low, high) = cone.contact(Vec2::new(2.2, 0.0), 0.45, -0.2, 2.2).unwrap();
        assert!((1.0..1.5).contains(&low), "{low}");
        assert!(
            (1.4..2.0).contains(&high),
            "{high}: the crown narrows above"
        );
        assert!(cone.contact(Vec2::new(0.0, 2.6), 0.45, -0.2, 2.2).is_none());
        assert!(
            cone.contact(Vec2::new(2.2, 0.0), 0.45, -0.2, 0.8).is_none(),
            "no snow below the crown's foot"
        );
        let trunk = cone.contact(Vec2::ZERO, 0.45, 2.0, 3.0).unwrap();
        assert_eq!(trunk, (2.0, 3.0));
        assert!(SnowProfile::of([Vec3::new(1.0, -1.0, 0.0)]).is_none());
    }

    fn shake(x: f64, low: f32, high: f32, time: f32) -> Shake {
        Shake {
            centre: DVec2::new(x, 10.0),
            low,
            high,
            time,
        }
    }

    #[test]
    fn only_heights_not_yet_shaken_shake_snow_loose() {
        let mut shed = SnowShed {
            bodies: vec![Body::default()],
            ..default()
        };
        shed.map.follow(DVec2::new(0.0, 10.0), &shed.log);
        let first = shed.shake(0, shake(0.0, 0.0, 2.0, 1.0));
        assert!(first > 1.0, "{first}");
        assert_eq!(shed.shake(0, shake(0.0, 0.5, 1.5, 2.0)), 0.0);
        let higher = shed.shake(0, shake(0.0, 0.0, 2.5, 3.0));
        assert!((higher - first / 4.0).abs() < 1e-3, "{higher}");
        let texel = shed.map.texels[index(0, 40)];
        assert_eq!(texel, [0.0, 2.5, 1.0, 0.0], "it fell when first shaken");
        assert_eq!(shed.log.len(), 1, "a body standing still is logged once");
    }

    #[test]
    fn the_map_is_drawn_again_from_the_log_round_the_player_and_uploads_what_changed() {
        let mut shed = SnowShed {
            bodies: vec![Body::default()],
            ..default()
        };
        let home = DVec2::new(0.0, 10.0);
        shed.map.follow(home, &shed.log);
        assert_eq!(shed.map.take_rows().len(), 1);
        assert_eq!(shed.map.take_rows().len(), 0);
        shed.shake(0, shake(0.0, 0.0, 2.0, 1.0));
        let rows = shed.map.take_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.len(), 6 * SIDE as usize, "the disc's rows");

        // Away: the shaken spot leaves the window and its texels hold other ground.
        let away = DVec2::new(100.0, 10.0);
        shed.map.follow(away, &shed.log);
        assert_eq!(shed.map.texels[index(0, 40)], UNSHAKEN);
        let none = shed.map.window([0.0; 2], 0.0).shaken;
        assert!(none[0] > none[2], "nothing shaken in the window");

        shed.map.follow(home, &shed.log);
        assert_eq!(shed.map.texels[index(0, 40)], [0.0, 2.0, 1.0, 0.0]);
        let window = shed.map.window([0.0; 2], 5.0);
        assert_eq!(window.area, [-32.0, -22.0, 0.25, 256.0]);
        assert!(window.shaken[0] < 0.0 && window.shaken[2] > 0.0);
    }

    #[test]
    fn rows_wrapping_past_the_last_upload_in_two_runs() {
        let mut map = ShedMap {
            first: Some(I64Vec2::ZERO),
            rows: Some((254, 257)),
            ..default()
        };
        let runs: Vec<_> = map
            .take_rows()
            .into_iter()
            .map(|(row, texels)| (row, texels.len() / SIDE as usize))
            .collect();
        assert_eq!(runs, [(254, 2), (0, 2)]);
    }
}
