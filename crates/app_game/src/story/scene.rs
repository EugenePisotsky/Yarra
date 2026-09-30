//! Bevy side of the story slice: where the guard and gate stand, which keys do what, and
//! the text on screen. Everything it decides is presentation; outcomes come from `Story`.
use super::{Panel, Story};
use bevy::prelude::*;
use engine::{
    GameplaySystems, PlayerControlled, PlayerMovementSuspended, TerrainGrounded, WorldCatalog,
    WorldOrigin, WorldRenderRoot, WorldStartAdopted, WorldStartView,
};
use std::path::Path;

const TALK_RANGE: f32 = 3.0;
const GUARD_DISTANCE: f32 = 6.0;
const GATE_DISTANCE: f32 = 11.0;
/// Height given to a newly placed actor; terrain grounding replaces it once the ground is known.
const UNGROUNDED: f32 = -5000.0;
/// Bearings tried around the start, nearest to the view direction first.
const BEARINGS: [f32; 8] = [0.0, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0, 180.0];

pub(crate) fn install(app: &mut App, source: &Path) -> Result<(), String> {
    let work = std::env::temp_dir().join("yarra-story");
    let story = Story::open(source, &work)?;
    info!(
        "Story slice: {} (saves in {})",
        source.display(),
        work.display()
    );
    app.world_mut().insert_non_send(story);
    app.init_resource::<Placement>()
        .add_systems(Startup, spawn)
        .add_systems(
            Update,
            (
                place.after(GameplaySystems::Grounding),
                // Before movement, so a conversation stops the player on the frame it opens.
                input.before(GameplaySystems::MoveIntent),
                present.after(GameplaySystems::Grounding),
            ),
        );
    Ok(())
}

#[derive(Component)]
struct Guard;
/// The party member who waits with the player; she speaks in conversations but does not
/// follow yet.
#[derive(Component)]
struct Companion;
#[derive(Component)]
struct Gate;
#[derive(Component)]
struct GateHinge;
#[derive(Component)]
struct SummaryText;
#[derive(Component)]
struct PanelBox;
#[derive(Component)]
struct SpeakerText;
#[derive(Component)]
struct BodyText;

#[derive(Resource, Default)]
struct Placement {
    /// Player position when the start became known, and the ground direction the view faces.
    anchor: Option<(Vec3, Vec2)>,
    /// Yaw of the start view the world supplied, once it has.
    yaw: Option<f32>,
    attempt: usize,
    /// Frames since the world opened without the world naming a start of its own.
    waited: u32,
    settled: bool,
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let hidden = Vec3::new(0.0, UNGROUNDED, 0.0);
    commands
        .spawn(engine::standing_character(hidden, "Gate guard"))
        .insert(Guard);
    commands
        .spawn(engine::standing_character(hidden, "Companion"))
        .insert(Companion);
    let stone = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.44, 0.42),
        perceptual_roughness: 0.95,
        ..default()
    });
    let wood = materials.add(StandardMaterial {
        base_color: Color::srgb(0.36, 0.23, 0.12),
        perceptual_roughness: 0.85,
        ..default()
    });
    let post = meshes.add(Cuboid::new(0.5, 3.2, 0.5));
    commands
        .spawn((
            Transform::from_translation(hidden),
            Visibility::Inherited,
            TerrainGrounded,
            WorldRenderRoot,
            Gate,
            Name::new("Old gate"),
        ))
        .with_children(|gate| {
            for x in [-1.75, 1.75] {
                gate.spawn((
                    Mesh3d(post.clone()),
                    MeshMaterial3d(stone.clone()),
                    Transform::from_xyz(x, 1.6, 0.0),
                ));
            }
            // The door hangs from the left post and swings about it.
            gate.spawn((
                Transform::from_xyz(-1.5, 0.0, 0.0),
                Visibility::Inherited,
                GateHinge,
            ))
            .with_child((
                Mesh3d(meshes.add(Cuboid::new(3.0, 2.6, 0.14))),
                MeshMaterial3d(wood),
                Transform::from_xyz(1.5, 1.35, 0.0),
            ));
        });

    let font = |size: f32| TextFont {
        font_size: FontSize::Px(size),
        ..default()
    };
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(24),
                right: px(24),
                bottom: px(24),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(8),
                ..default()
            },
            GlobalZIndex(50),
        ))
        .with_children(|root| {
            root.spawn((
                PanelBox,
                Visibility::Hidden,
                Node {
                    width: px(720),
                    max_width: percent(100),
                    padding: UiRect::all(px(18)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(8),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.04, 0.05, 0.88)),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new(""),
                    font(17.0),
                    TextColor(Color::srgb(1.0, 0.78, 0.3)),
                    SpeakerText,
                ));
                panel.spawn((Text::new(""), font(19.0), BodyText));
            });
            root.spawn((
                Text::new(""),
                font(13.0),
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
                SummaryText,
            ));
        });
}

/// Stands the guard and the gate a few steps from where the player starts, in the direction
/// the start view faces. A spot that turns out to be under water is abandoned for the next
/// bearing. The engine keeps grounded actors hidden until the terrain under them is ready.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn place(
    mut placement: ResMut<Placement>,
    start_view: Res<WorldStartView>,
    mut adopted: MessageReader<WorldStartAdopted>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    player: Single<
        &Transform,
        (
            With<PlayerControlled>,
            Without<Guard>,
            Without<Gate>,
            Without<Companion>,
        ),
    >,
    mut guard: Single<&mut Transform, (With<Guard>, Without<Gate>, Without<Companion>)>,
    mut gate: Single<&mut Transform, (With<Gate>, Without<Guard>, Without<Companion>)>,
    mut companion: Single<&mut Transform, (With<Companion>, Without<Guard>, Without<Gate>)>,
) {
    // Read every frame: the world may name its start before the space is known here.
    if let Some(adopted) = adopted.read().last() {
        placement.yaw = Some(adopted.0.yaw_degrees.to_radians());
    }
    if placement.settled {
        return;
    }
    let Some(space) = origin.space() else {
        return;
    };
    if placement.anchor.is_none() {
        placement.waited += 1;
        let yaw = placement
            .yaw
            .or(start_view.0.as_ref().map(|v| v.yaw_degrees.to_radians()));
        let yaw = match yaw {
            Some(yaw) => yaw,
            // A world without a start of its own: use where the player stands.
            None if placement.waited > 240 => 0.0,
            None => return,
        };
        // The camera sits at +(sin yaw, cos yaw) from the player and looks back at it.
        placement.anchor = Some((player.translation, -Vec2::new(yaw.sin(), yaw.cos())));
        placement.attempt = 0;
        position(&placement, &mut guard, &mut gate, &mut companion);
        return;
    }
    if guard.translation.y == UNGROUNDED || gate.translation.y == UNGROUNDED {
        return; // terrain under them has not streamed in yet
    }
    let sea = catalog.world_space(space).and_then(|s| s.sea_level);
    let anchor = placement.anchor.expect("anchor set above").0;
    let usable = |y: f32| sea.is_none_or(|sea| y > sea + 0.3) && (y - anchor.y).abs() < 8.0;
    let last = placement.attempt + 1 == BEARINGS.len();
    if usable(guard.translation.y) && usable(gate.translation.y) || last {
        placement.settled = true;
    } else {
        placement.attempt += 1;
        position(&placement, &mut guard, &mut gate, &mut companion);
    }
}
fn position(
    placement: &Placement,
    guard: &mut Transform,
    gate: &mut Transform,
    companion: &mut Transform,
) {
    let (anchor, forward) = placement.anchor.expect("anchor known");
    let direction = Vec2::from_angle(BEARINGS[placement.attempt].to_radians()).rotate(forward);
    let side = direction.perp();
    let at = |along: f32, across: f32| {
        let ground = Vec2::new(anchor.x, anchor.z) + direction * along + side * across;
        Vec3::new(ground.x, UNGROUNDED, ground.y)
    };
    let facing = |toward: Vec2| Quat::from_rotation_y(f32::atan2(toward.x, toward.y));
    // The guard waits beside the path, clear of the door's swing, and looks back at the
    // player; the gate spans the path.
    *guard =
        Transform::from_translation(at(GUARD_DISTANCE, -2.6)).with_rotation(facing(-direction));
    *gate = Transform::from_translation(at(GATE_DISTANCE, 0.0)).with_rotation(facing(direction));
    *companion = Transform::from_translation(at(1.0, 1.8)).with_rotation(facing(direction));
}

fn near_guard(player: &Transform, guard: &Transform) -> bool {
    player.translation.distance(guard.translation) < TALK_RANGE
}

fn input(
    mut story: NonSendMut<Story>,
    keys: Res<ButtonInput<KeyCode>>,
    placement: Res<Placement>,
    player: Single<&Transform, (With<PlayerControlled>, Without<Guard>)>,
    guard: Single<&Transform, With<Guard>>,
) {
    if keys.just_pressed(KeyCode::F5) {
        story.quicksave();
    }
    if keys.just_pressed(KeyCode::F9) {
        story.quickload();
    }
    if !story.in_conversation() {
        if keys.just_pressed(KeyCode::KeyE) && placement.settled && near_guard(&player, &guard) {
            story.talk();
        }
        return;
    }
    if keys.any_just_pressed([KeyCode::Space, KeyCode::Enter]) {
        story.advance();
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        story.leave();
    }
    const DIGITS: [KeyCode; 9] = [
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
    if let Some(index) = DIGITS.iter().position(|key| keys.just_pressed(*key)) {
        story.choose(index);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn present(
    mut story: NonSendMut<Story>,
    time: Res<Time>,
    placement: Res<Placement>,
    mut suspended: ResMut<PlayerMovementSuspended>,
    player: Single<&Transform, (With<PlayerControlled>, Without<Guard>, Without<GateHinge>)>,
    guard: Single<&Transform, (With<Guard>, Without<GateHinge>)>,
    mut hinge: Single<&mut Transform, With<GateHinge>>,
    mut panel_box: Single<&mut Visibility, With<PanelBox>>,
    mut texts: ParamSet<(
        Single<&mut Text, With<SpeakerText>>,
        Single<&mut Text, With<BodyText>>,
        Single<&mut Text, With<SummaryText>>,
    )>,
    mut shown: Local<Option<(u64, bool)>>,
) {
    suspended.0 = story.in_conversation();

    // An unlocked gate swings open; a reload that locks it again swings it shut.
    let target = if story.gate_locked() { 0.0 } else { 1.9 };
    let (current, _, _) = hinge.rotation.to_euler(EulerRot::YXZ);
    let step = (target - current).clamp(-1.0, 1.0) * (time.delta_secs() * 3.0).min(1.0);
    if step.abs() > 1e-4 {
        hinge.rotation = Quat::from_rotation_y(current + step);
    }

    // Text is rebuilt only after a command, a load, or crossing the talk range.
    let near = placement.settled && near_guard(&player, &guard);
    let current = (story.revision(), near);
    if *shown == Some(current) {
        return;
    }
    *shown = Some(current);
    let (speaker, body) = match story.panel() {
        Some(Panel {
            speaker,
            text,
            choices,
        }) if choices.is_empty() => (speaker, format!("{text}\n\n[Space] Continue    [Q] Leave")),
        Some(Panel {
            speaker, choices, ..
        }) => {
            let list: Vec<String> = choices
                .iter()
                .enumerate()
                .map(|(i, choice)| format!("{}. {choice}", i + 1))
                .collect();
            (speaker, list.join("\n"))
        }
        None if near => (String::new(), format!("[E] Talk to {}", story.guard_name())),
        None => (String::new(), String::new()),
    };
    // A conversation that just ended releases the player.
    suspended.0 = story.in_conversation();
    **panel_box = if body.is_empty() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    let mut summary = story.summary();
    summary.push_str("    [F5] Save  [F9] Load");
    if !story.notice.is_empty() {
        summary = format!("{}\n{summary}", story.notice);
    }
    texts.p0().0 = speaker;
    texts.p1().0 = body;
    texts.p2().0 = summary;
}
