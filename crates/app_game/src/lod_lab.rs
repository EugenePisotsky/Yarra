//! LOD lab (`--lod-lab ASSET --start-view VIEW`): the tree under study, optionally inside a
//! stand of neighbours, stands at the start view's focus outside streaming
//! ([`engine::spawn_lab_tree`]) and is drawn with the same lighting, AA and LOD projection
//! as play. The camera looks at it from a bearing relative to the sun, at a distance from
//! its root (as LOD distances are measured), and its representation can be forced.
//!
//! A [`panel`] sets the view, the representation, the sun and shadows. Keys: Up/Down
//! distance, Left/Right bearing, PageUp/PageDown pitch, 0 the game's choice, 1–9 mesh LOD,
//! I impostor, H hidden, B blink between the last two forced, F the next switch.
//! `--lod-lab-capture DIR` instead runs [`capture`] and exits.
mod capture;
mod panel;

use crate::launch::{LaunchOptions, LodLabOptions};
use crate::runtime_settings::RuntimeSettings;
use bevy::prelude::*;
use engine::{
    GameplaySystems, LabAsset, LabRepresentation, LabTree, PlayerControlled,
    StreamedTerrainSurface, WorldCatalog, WorldOrigin, WorldStartView, WorldSun, WorldViewCamera,
};
use std::{path::Path, sync::Arc};

/// Ordering for systems that read the lab's view after it is applied.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LabCameraSet;

#[derive(Resource)]
pub(crate) struct LodLab {
    options: LodLabOptions,
    subject: Arc<LabAsset>,
    stand: Option<Arc<LabAsset>>,
    /// The tree under study, once placed, and its render-space root.
    tree: Option<(Entity, Vec3)>,
    neighbours: Vec<Entity>,
    view: LabView,
    /// The two representations B flips between, latest last.
    forced: [LabRepresentation; 2],
    blink: Option<Timer>,
    waiting_since: Option<f32>,
    /// The wind blows (the panel, sway captures); otherwise it is held still.
    wind: bool,
}

/// Where the camera looks from: a bearing from the sun's (0 puts the sun behind the
/// camera), an elevation, and the distance to the tree's root.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LabView {
    pub(crate) bearing: f32,
    pub(crate) pitch: f32,
    pub(crate) distance: f32,
}

pub(crate) fn install(app: &mut App) -> Result<(), String> {
    let Some(options) = app.world().resource::<LaunchOptions>().lod_lab.clone() else {
        return Ok(());
    };
    let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/packs");
    let subject = Arc::new(LabAsset::find(&packs, &options.asset)?);
    let stand = options
        .stand
        .as_ref()
        .map(|(key, ..)| LabAsset::find(&packs, key).map(Arc::new))
        .transpose()?;
    {
        let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
        settings.controls_locked = true;
        if options.capture.is_some() {
            settings.show_ui = false;
        }
    }
    let view = LabView {
        bearing: options.yaws[0].to_radians(),
        pitch: options.pitch.to_radians(),
        distance: options.distances[0].max(subject.bounds[1] * 1.5),
    };
    let capture = options.capture.clone();
    app.add_plugins(engine::LodLabPlugin)
        .insert_resource(LodLab {
            options,
            subject,
            stand,
            tree: None,
            neighbours: Vec::new(),
            view,
            forced: [LabRepresentation::Auto; 2],
            blink: None,
            waiting_since: None,
            wind: false,
        })
        .add_systems(
            Update,
            (place, hide_player, drive_camera)
                .chain()
                .in_set(LabCameraSet)
                .after(GameplaySystems::CameraFollow),
        )
        .add_systems(PostUpdate, freeze_wind.before(engine::TreeWindSystems));
    match capture {
        Some(dir) => capture::install(app, dir),
        None => {
            panel::install(app);
            app.add_systems(Update, (keys, blink).chain().before(LabCameraSet));
            if let Some(path) = app.world().resource::<LodLab>().options.screenshot.clone() {
                app.insert_resource(Snapshot { path, frames: 0 })
                    .add_systems(Update, snapshot.after(LabCameraSet));
            }
        }
    }
    Ok(())
}

/// Stand positions around the tree: a hexagonal grid at `spacing`, nearest first and
/// jittered by up to a quarter of it, each with a yaw and a scale of 0.9–1.1.
fn stand_layout(count: usize, spacing: f32) -> Vec<(Vec2, f32, f32)> {
    let mut random = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        (random >> 40) as f32 / (1u64 << 24) as f32
    };
    let rings = (count as f32).sqrt().ceil() as i32 + 2;
    let mut points = Vec::new();
    for q in -rings..=rings {
        for r in -rings..=rings {
            if (q, r) == (0, 0) {
                continue;
            }
            let x = spacing * (q as f32 + r as f32 * 0.5);
            let z = spacing * r as f32 * 3f32.sqrt() * 0.5;
            points.push(Vec2::new(x, z));
        }
    }
    points.sort_by(|a, b| a.length().total_cmp(&b.length()));
    points
        .into_iter()
        .take(count)
        .map(|p| {
            let jitter = (Vec2::new(next(), next()) - 0.5) * spacing * 0.5;
            (
                p + jitter,
                next() * std::f32::consts::TAU,
                0.9 + 0.2 * next(),
            )
        })
        .collect()
}

/// Places the trees once the ground under all of them is resident.
#[allow(clippy::too_many_arguments)]
fn place(
    mut commands: Commands,
    mut lab: ResMut<LodLab>,
    server: Res<AssetServer>,
    start: Res<WorldStartView>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    surfaces: Query<&StreamedTerrainSurface>,
    time: Res<Time<Real>>,
) {
    if lab.tree.is_some() {
        return;
    }
    let (Some(view), Some(space)) = (start.0.as_ref(), origin.space()) else {
        return;
    };
    let cell_size = catalog
        .world_space(space)
        .map_or(world::DEFAULT_CELL_SIZE, |s| s.cell_size);
    let offset = origin.cell().origin(cell_size);
    let focus = Vec2::new(
        (f64::from(view.position[0]) - offset[0]) as f32,
        (f64::from(view.position[2]) - offset[1]) as f32,
    );
    let mut trees = vec![(focus, 0.0, lab.options.scale, lab.subject.clone())];
    if let (Some(stand), Some((_, count, spacing))) = (&lab.stand, &lab.options.stand) {
        trees.extend(
            stand_layout(*count, *spacing)
                .into_iter()
                .map(|(p, yaw, scale)| (focus + p, yaw, scale, stand.clone())),
        );
    }
    let heights: Option<Vec<f32>> = trees
        .iter()
        .map(|(p, ..)| {
            engine::sample_resident_terrain_surface(&origin, surfaces.iter(), [p.x, p.y])
                .map(|s| s.height)
        })
        .collect();
    let Some(heights) = heights else {
        let since = *lab.waiting_since.get_or_insert(time.elapsed_secs());
        if time.elapsed_secs() - since > 60.0 {
            panic!("LOD lab: the ground under the trees never became resident");
        }
        return;
    };
    let mut placed = trees
        .into_iter()
        .zip(heights)
        .map(|((p, yaw, scale, asset), y)| {
            let root = Vec3::new(p.x, y, p.y);
            let entity = engine::spawn_lab_tree(
                &mut commands,
                &server,
                asset,
                Transform::from_translation(root)
                    .with_rotation(Quat::from_rotation_y(yaw))
                    .with_scale(Vec3::splat(scale)),
            );
            (entity, root)
        });
    lab.tree = placed.next();
    lab.neighbours = placed.map(|(entity, _)| entity).collect();
    info!(
        "LOD_LAB placed {} with {} neighbours at {:?}",
        lab.subject.key,
        lab.neighbours.len(),
        lab.tree.map(|t| t.1)
    );
}

fn hide_player(mut players: Query<&mut Visibility, With<PlayerControlled>>) {
    for mut visibility in &mut players {
        visibility.set_if_neq(Visibility::Hidden);
    }
}

/// The horizontal bearing towards the sun, in radians from +Z towards +X.
fn sun_bearing(sun: &GlobalTransform) -> f32 {
    let to_sun = sun.back();
    to_sun.x.atan2(to_sun.z)
}

/// The camera on the lab's view of a tree rooted at `root`, `height` tall.
fn camera_pose(view: LabView, sun_bearing: f32, root: Vec3, height: f32) -> Transform {
    let bearing = sun_bearing + view.bearing;
    let direction = Vec3::new(
        bearing.sin() * view.pitch.cos(),
        view.pitch.sin(),
        bearing.cos() * view.pitch.cos(),
    );
    Transform::from_translation(root + direction * view.distance)
        .looking_at(root + Vec3::Y * height * 0.5, Vec3::Y)
}

fn drive_camera(
    lab: Res<LodLab>,
    trees: Query<&GlobalTransform, With<LabTree>>,
    sun: Single<&GlobalTransform, With<WorldSun>>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
) {
    let Some((entity, root)) = lab.tree else {
        return;
    };
    let scale = trees
        .get(entity)
        .map_or(1.0, |t| t.to_scale_rotation_translation().0.y);
    **camera = camera_pose(
        lab.view,
        sun_bearing(&sun),
        root,
        lab.subject.bounds[1] * scale,
    );
}

/// Wind still unless asked for, so frames differ only by what the lab changes.
fn freeze_wind(lab: Res<LodLab>, mut wind: ResMut<vegetation_render::VegetationWind>) {
    if !lab.wind {
        wind.set_phase_seconds(0.0);
    }
}

fn label(representation: LabRepresentation) -> String {
    match representation {
        LabRepresentation::Auto => "auto".into(),
        LabRepresentation::Mesh(lod) => format!("LOD{lod}"),
        LabRepresentation::Impostor => "impostor".into(),
        LabRepresentation::Hidden => "hidden".into(),
    }
}

/// Distances where the representation the game draws changes: the middle of each band's
/// fade-out, nearest first.
fn switches(tree: &LabTree) -> Vec<f32> {
    tree.bands
        .iter()
        .filter(|b| b.fade_out.end < f32::MAX)
        .map(|b| (b.fade_out.start + b.fade_out.end) * 0.5)
        .collect()
}

fn keys(
    input: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    mut lab: ResMut<LodLab>,
    mut trees: Query<(&LabTree, &mut LabRepresentation)>,
) {
    let Some((entity, _)) = lab.tree else {
        return;
    };
    let Ok((tree, mut representation)) = trees.get_mut(entity) else {
        return;
    };
    let dt = time.delta_secs();
    let mut view = lab.view;
    if input.pressed(KeyCode::ArrowUp) {
        view.distance /= 1.0 + dt * 0.8;
    }
    if input.pressed(KeyCode::ArrowDown) {
        view.distance *= 1.0 + dt * 0.8;
    }
    if input.pressed(KeyCode::ArrowLeft) {
        view.bearing -= dt;
    }
    if input.pressed(KeyCode::ArrowRight) {
        view.bearing += dt;
    }
    if input.pressed(KeyCode::PageUp) {
        view.pitch = (view.pitch + dt * 0.5).min(1.4);
    }
    if input.pressed(KeyCode::PageDown) {
        view.pitch = (view.pitch - dt * 0.5).max(-0.2);
    }
    if input.just_pressed(KeyCode::KeyF) {
        let next = switches(tree)
            .into_iter()
            .find(|&s| s > view.distance * 1.01)
            .or_else(|| switches(tree).first().copied());
        if let Some(switch) = next {
            view.distance = switch;
        }
    }
    view.distance = view.distance.clamp(2.0, 4000.0);
    lab.view = view;
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    let mut chosen = None;
    if input.just_pressed(KeyCode::Digit0) {
        chosen = Some(LabRepresentation::Auto);
    }
    for (lod, key) in digits.iter().enumerate() {
        if input.just_pressed(*key) && lod < tree.asset.meshes() {
            chosen = Some(LabRepresentation::Mesh(lod));
        }
    }
    if input.just_pressed(KeyCode::KeyI) && tree.asset.impostor().is_some() {
        chosen = Some(LabRepresentation::Impostor);
    }
    if input.just_pressed(KeyCode::KeyH) {
        chosen = Some(LabRepresentation::Hidden);
    }
    if let Some(chosen) = chosen {
        lab.blink = None;
        if chosen != LabRepresentation::Auto && chosen != lab.forced[1] {
            lab.forced = [lab.forced[1], chosen];
        }
        representation.set_if_neq(chosen);
    }
    if input.just_pressed(KeyCode::KeyB) {
        lab.blink = match lab.blink {
            Some(_) => None,
            None => Some(Timer::from_seconds(0.5, TimerMode::Repeating)),
        };
    }
}

fn blink(
    time: Res<Time<Real>>,
    mut lab: ResMut<LodLab>,
    mut trees: Query<&mut LabRepresentation, With<LabTree>>,
) {
    let Some((entity, _)) = lab.tree else {
        return;
    };
    let forced = lab.forced;
    let Some(timer) = lab.blink.as_mut() else {
        return;
    };
    if !timer.tick(time.delta()).just_finished() {
        return;
    }
    if let Ok(mut representation) = trees.get_mut(entity) {
        let next = if *representation == forced[1] {
            forced[0]
        } else {
            forced[1]
        };
        representation.set_if_neq(next);
    }
}

#[derive(Resource)]
struct Snapshot {
    path: std::path::PathBuf,
    frames: u32,
}

/// `--lod-lab-screenshot`: the window once every tree has drawn for four seconds, then exit.
fn snapshot(
    mut commands: Commands,
    mut snapshot: ResMut<Snapshot>,
    trees: Query<&LabTree>,
    mut exit: MessageWriter<AppExit>,
) {
    if trees.is_empty() || !trees.iter().all(|t| t.ready) {
        return;
    }
    snapshot.frames += 1;
    if snapshot.frames == 240 {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(snapshot.path.clone()));
    }
    if snapshot.frames == 300 {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_layout_is_nearest_first_and_leaves_the_centre_free() {
        let layout = stand_layout(24, 6.0);
        assert_eq!(layout.len(), 24);
        for (p, yaw, scale) in &layout {
            assert!(p.length() >= 6.0 * 0.6, "{p:?} crowds the tree under study");
            assert!((0.0..std::f32::consts::TAU).contains(yaw));
            assert!((0.9..=1.1).contains(scale));
        }
        assert_eq!(layout, stand_layout(24, 6.0));
    }

    #[test]
    fn camera_pose_keeps_its_distance_to_the_root_and_faces_the_crown() {
        let view = LabView {
            bearing: 0.3,
            pitch: 0.05,
            distance: 150.0,
        };
        let root = Vec3::new(10.0, 4.0, -3.0);
        let camera = camera_pose(view, 1.2, root, 14.0);
        assert!((camera.translation.distance(root) - 150.0).abs() < 1e-3);
        let towards = (root + Vec3::Y * 7.0 - camera.translation).normalize();
        assert!(camera.forward().dot(towards) > 0.9999);
    }
}
