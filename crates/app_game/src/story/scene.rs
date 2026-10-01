//! Bevy side of the story slice: where the guard and gate stand, which keys do what, the
//! text on screen, and the engine's half of the world rules: telling them which named areas
//! the party stands in and walking the actors they ask to move. Everything it decides is
//! presentation; outcomes come from `Story`.
use super::{Bark, DUMMY, GUARD, HERO, MIRA, Panel, Story};
use bevy::prelude::*;
use engine::{
    GameplaySystems, MoveIntent, PlayerControlled, PlayerMovementSuspended, TerrainGrounded,
    WorldCatalog, WorldOrigin, WorldRenderRoot, WorldStartAdopted, WorldStartView,
};
use game_types::{ActorId, AreaId};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use world::{GameplayArea, GameplayAreaIndex, WorldSpaceId};

const TALK_RANGE: f32 = 3.0;
/// How close the player stands to the dummy to hit it, and how far away gives it up.
const STRIKE_RANGE: f32 = 2.5;
const DISENGAGE_RANGE: f32 = 4.0;
const GUARD_DISTANCE: f32 = 6.0;
const GATE_DISTANCE: f32 = 11.0;
/// Height given to a newly placed actor; terrain grounding replaces it once the ground is known.
const UNGROUNDED: f32 = -5000.0;
/// Bearings tried around the start, nearest to the view direction first.
const BEARINGS: [f32; 8] = [0.0, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0, 180.0];
/// Shapes for the areas the demo content names, used while the world has none painted under
/// those names: metres along the path from the start, then across it.
const STAND_INS: [(&str, [f32; 2], [f32; 2]); 2] = [
    ("guard/approach", [2.5, 10.0], [-6.0, 5.0]),
    ("guard/gate_post", [9.2, 11.6], [-4.6, -2.4]),
];

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
        .init_resource::<Places>()
        .add_systems(Startup, spawn)
        .add_systems(
            Update,
            (
                // Before movement, so a conversation stops the player on the frame it opens.
                (adopt_player, input, restore)
                    .chain()
                    .before(GameplaySystems::MoveIntent),
                walk.after(GameplaySystems::MoveIntent)
                    .before(GameplaySystems::Movement),
                (place, observe, present)
                    .chain()
                    .after(GameplaySystems::Grounding),
            ),
        );
    Ok(())
}

/// Which actor of the playthrough an entity is.
#[derive(Component)]
struct Actor(ActorId);
#[derive(Component)]
struct Guard;
/// The party member who waits with the player; she speaks in conversations but does not
/// follow yet.
#[derive(Component)]
struct Companion;
/// Something to hit.
#[derive(Component)]
struct Dummy;
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
#[derive(Component)]
struct BarkText;

#[derive(Resource, Default)]
struct Placement {
    /// World position of the player when the start became known, and the ground direction
    /// the view faces.
    anchor: Option<([f64; 3], Vec2)>,
    /// Yaw of the start view the world supplied, once it has.
    yaw: Option<f32>,
    attempt: usize,
    /// Frames since the world opened without the world naming a start of its own.
    waited: u32,
    settled: bool,
}
impl Placement {
    /// The world ground position a distance along the path from the start and across it.
    fn ground(&self, along: f32, across: f32) -> [f64; 2] {
        let (anchor, _) = self.anchor.expect("anchor known");
        let direction = self.direction();
        let offset = direction * along + direction.perp() * across;
        [
            anchor[0] + f64::from(offset.x),
            anchor[2] + f64::from(offset.y),
        ]
    }
    fn direction(&self) -> Vec2 {
        let (_, forward) = self.anchor.expect("anchor known");
        Vec2::from_angle(BEARINGS[self.attempt].to_radians()).rotate(forward)
    }
}

/// The shapes behind the area names the content uses: those painted in the world, and
/// stand-ins beside the guard for the demo's names the world does not have.
#[derive(Resource, Default)]
struct Places {
    index: GameplayAreaIndex,
    ids: HashMap<String, AreaId>,
    /// Where an actor sent to an area walks to, in the area's world.
    targets: HashMap<AreaId, (WorldSpaceId, [f64; 2])>,
}
impl Places {
    fn new(story: &Story, catalog: &WorldCatalog, origin: &WorldOrigin, at: &Placement) -> Self {
        let mut areas: Vec<GameplayArea> = catalog
            .gameplay_areas()
            .areas()
            .iter()
            .filter(|area| story.area(&area.name).is_some())
            .cloned()
            .collect();
        let painted = areas.len();
        if let Some(space) = origin.space() {
            for (name, along, across) in STAND_INS {
                if story.area(name).is_none() || areas.iter().any(|area| area.name == name) {
                    continue;
                }
                let corners = [(0, 0), (1, 0), (1, 1), (0, 1)];
                areas.push(GameplayArea {
                    name: name.into(),
                    space,
                    points: corners.map(|(i, j)| at.ground(along[i], across[j])).into(),
                    height: None,
                });
            }
        }
        info!(
            "Story areas: {painted} painted in the world, {} stand-ins",
            areas.len() - painted
        );
        let ids: HashMap<String, AreaId> = areas
            .iter()
            .filter_map(|area| Some((area.name.clone(), story.area(&area.name)?)))
            .collect();
        let targets = areas
            .iter()
            .map(|area| (ids[&area.name], (area.space, area.interior_point())))
            .collect();
        // Content that sends someone to an area the world lacks would fail every such walk.
        for missing in story
            .declared_areas()
            .filter(|id| !ids.values().any(|v| v == id))
        {
            warn!("Story: the world has no area named {missing}");
        }
        Self {
            index: GameplayAreaIndex::new(Arc::from(areas)),
            ids,
            targets,
        }
    }
}

fn adopt_player(
    mut commands: Commands,
    player: Query<Entity, (With<PlayerControlled>, Without<Actor>)>,
) {
    for player in &player {
        commands.entity(player).insert(Actor(HERO));
    }
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let hidden = Vec3::new(0.0, UNGROUNDED, 0.0);
    commands
        .spawn(engine::standing_character(hidden, "Gate guard"))
        .insert((Guard, Actor(GUARD)));
    commands
        .spawn(engine::standing_character(hidden, "Companion"))
        .insert((Companion, Actor(MIRA)));
    commands
        .spawn(engine::standing_character(hidden, "Training dummy"))
        .insert((Dummy, Actor(DUMMY)));
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
            // Lines spoken while play goes on; nothing waits for them.
            root.spawn((
                Text::new(""),
                font(17.0),
                TextColor(Color::srgb(0.95, 0.93, 0.85)),
                TextShadow {
                    offset: Vec2::splat(1.5),
                    color: Color::BLACK.with_alpha(0.85),
                },
                BarkText,
            ));
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
    story: NonSend<Story>,
    mut placement: ResMut<Placement>,
    mut places: ResMut<Places>,
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
    mut dummy: Single<
        &mut Transform,
        (
            With<Dummy>,
            Without<Guard>,
            Without<Gate>,
            Without<Companion>,
            Without<PlayerControlled>,
        ),
    >,
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
    // Taken before anything moves: a bearing is judged by where its actors came to rest.
    let heights = [guard.translation.y, gate.translation.y];
    let mut position = |placement: &Placement| {
        [**guard, **gate, **companion, **dummy] = stand(placement, &origin, &catalog);
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
        let Some((_, anchor)) = origin.to_world(&catalog, player.translation) else {
            return;
        };
        // The camera sits at +(sin yaw, cos yaw) from the player and looks back at it.
        placement.anchor = Some((anchor, -Vec2::new(yaw.sin(), yaw.cos())));
        placement.attempt = 0;
        position(&placement);
        return;
    }
    if heights.contains(&UNGROUNDED) {
        return; // terrain under them has not streamed in yet
    }
    let sea = catalog.world_space(space).and_then(|s| s.sea_level);
    let anchor = placement.anchor.expect("anchor set above").0[1] as f32;
    let usable = |y: f32| sea.is_none_or(|sea| y > sea + 0.3) && (y - anchor).abs() < 8.0;
    let last = placement.attempt + 1 == BEARINGS.len();
    if heights.into_iter().all(usable) || last {
        placement.settled = true;
        info!(
            "Story scene placed {}° from the start view's heading",
            BEARINGS[placement.attempt]
        );
        *places = Places::new(&story, &catalog, &origin, &placement);
    } else {
        placement.attempt += 1;
        position(&placement);
    }
}

/// Where the guard, the gate and the companion stand for the bearing being tried.
fn stand(placement: &Placement, origin: &WorldOrigin, catalog: &WorldCatalog) -> [Transform; 4] {
    let at = |along: f32, across: f32| {
        let [x, z] = placement.ground(along, across);
        let space = origin.space();
        space
            .and_then(|space| origin.to_render(catalog, space, [x, f64::from(UNGROUNDED), z]))
            .unwrap_or(Vec3::new(0., UNGROUNDED, 0.))
    };
    let direction = placement.direction();
    let facing = |toward: Vec2| Quat::from_rotation_y(f32::atan2(toward.x, toward.y));
    [
        // The guard waits beside the path, clear of the door's swing, and looks back at the
        // player; the gate spans the path.
        Transform::from_translation(at(GUARD_DISTANCE, -2.6)).with_rotation(facing(-direction)),
        Transform::from_translation(at(GATE_DISTANCE, 0.0)).with_rotation(facing(direction)),
        Transform::from_translation(at(1.0, 1.8)).with_rotation(facing(direction)),
        // The dummy stands off the path on the other side from the guard.
        Transform::from_translation(at(7.0, 4.5)).with_rotation(facing(-direction)),
    ]
}

/// Tells the rules where an actor is: its position and the named areas it stands in.
fn report(
    story: &mut Story,
    places: &Places,
    origin: &WorldOrigin,
    catalog: &WorldCatalog,
    actor: ActorId,
    at: Vec3,
) {
    let Some((space, position)) = origin.to_world(catalog, at) else {
        return;
    };
    let areas: BTreeSet<AreaId> = {
        let inside = story.areas(actor);
        places
            .index
            .at(space, position, |name| {
                places.ids.get(name).is_some_and(|id| inside.contains(id))
            })
            .filter_map(|area| places.ids.get(&area.name).copied())
            .collect()
    };
    if story.areas(actor) != &areas {
        let names: Vec<String> = areas.iter().map(AreaId::to_string).collect();
        info!("Story: {actor} is now in [{}]", names.join(", "));
    }
    story.observe(actor, position, areas);
}

/// Keeps the rules informed about the actors they follow, and about anyone the engine is
/// still walking: a walk the rules saw arrive goes on to the middle of its area, and what
/// the rules last heard must stay true. They hear about it only when an actor crosses into
/// or out of an area.
fn observe(
    mut story: NonSendMut<Story>,
    placement: Res<Placement>,
    places: Res<Places>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    actors: Query<(&Actor, &Transform, Option<&MoveIntent>)>,
) {
    if !placement.settled {
        return;
    }
    for (actor, transform, intent) in &actors {
        let walking = intent.is_some_and(|intent| intent.destination().is_some());
        if (walking || story.observed(actor.0)) && transform.translation.y != UNGROUNDED {
            let at = transform.translation;
            report(&mut story, &places, &origin, &catalog, actor.0, at);
        }
    }
}

/// Walks the actors the rules asked to move towards the middle of their area. The rules
/// decide when they have arrived, from the reports `observe` sends.
fn walk(
    mut story: NonSendMut<Story>,
    placement: Res<Placement>,
    places: Res<Places>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut actors: Query<(&Actor, &mut MoveIntent), Without<PlayerControlled>>,
    mut walking: Local<HashMap<ActorId, AreaId>>,
) {
    if !placement.settled {
        return;
    }
    // A walk for an actor the scene has no body for, or for the player, cannot happen.
    let drivable: HashSet<ActorId> = actors.iter().map(|(actor, _)| actor.0).collect();
    let stranded: Vec<(ActorId, u64)> = story
        .movements()
        .filter(|m| !drivable.contains(&m.actor))
        .map(|m| (m.actor, m.request))
        .collect();
    for (actor, request) in stranded {
        info!("Story: nothing here can walk {actor}");
        story.move_failed(actor, request);
    }
    for (actor, mut intent) in &mut actors {
        let Some((to, request)) = story.movement(actor.0).map(|m| (m.to, m.request)) else {
            // A walk that arrived carries on to the middle of its area; one that was given
            // up stops where it is.
            if let Some(to) = walking.remove(&actor.0) {
                let arrived = story.areas(actor.0).contains(&to);
                info!(
                    "Story: {} {} {to}",
                    actor.0,
                    if arrived { "reached" } else { "gave up on" }
                );
                if !arrived {
                    intent.clear();
                }
            }
            continue;
        };
        let target = places
            .targets
            .get(&to)
            .and_then(|&(space, [x, z])| origin.to_render(&catalog, space, [x, 0., z]));
        let Some(target) = target else {
            // Nobody painted the place the content sends this actor to, in this world.
            story.move_failed(actor.0, request);
            continue;
        };
        if walking.insert(actor.0, to) != Some(to) {
            info!("Story: {} walks to {to}", actor.0);
        }
        if story.in_conversation() {
            // The world waits for a conversation.
            if intent.destination().is_some() {
                intent.clear();
            }
        } else if intent
            .destination()
            .is_none_or(|current| current.xz().distance(target.xz()) > 0.05)
        {
            intent.walk_to(target);
        }
    }
}

/// After a load, stands every actor where the save recorded it.
fn restore(
    story: NonSend<Story>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut actors: Query<(&Actor, &mut Transform, &mut MoveIntent)>,
    mut seen: Local<u64>,
) {
    if *seen == story.loads() {
        return;
    }
    *seen = story.loads();
    for (actor, mut transform, mut intent) in &mut actors {
        // Saved positions do not name a world yet; they are in the one the game is in.
        let saved = story.position(actor.0).zip(origin.space());
        let saved = saved.and_then(|(position, space)| origin.to_render(&catalog, space, position));
        if let Some(position) = saved {
            transform.translation = position;
            intent.clear();
        }
    }
}

fn near_guard(player: &Transform, guard: &Transform) -> bool {
    player.translation.distance(guard.translation) < TALK_RANGE
}
fn within(player: &Transform, other: &Transform, range: f32) -> bool {
    player.translation.distance(other.translation) < range
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn input(
    mut story: NonSendMut<Story>,
    mut suspended: ResMut<PlayerMovementSuspended>,
    keys: Res<ButtonInput<KeyCode>>,
    placement: Res<Placement>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    player: Single<&Transform, (With<PlayerControlled>, Without<Guard>)>,
    guard: Single<&Transform, With<Guard>>,
    dummy: Single<&Transform, With<Dummy>>,
    actors: Query<(&Actor, &Transform)>,
) {
    // Saves hold where everyone stands, so both wait until the scene is in place.
    if keys.just_pressed(KeyCode::F5) && placement.settled {
        let positions: BTreeMap<ActorId, [f64; 3]> = actors
            .iter()
            .filter_map(|(actor, transform)| {
                let (_, position) = origin.to_world(&catalog, transform.translation)?;
                Some((actor.0, position))
            })
            .collect();
        story.record(positions);
        story.quicksave();
    }
    if keys.just_pressed(KeyCode::F9) && placement.settled {
        story.quickload();
    }
    handle(&mut story, &keys, &placement, &player, &guard, &dummy);
    // Before movement runs, so a conversation holds the player on the frame it opens.
    suspended.hold("conversation", story.in_conversation());
}

fn handle(
    story: &mut Story,
    keys: &ButtonInput<KeyCode>,
    placement: &Placement,
    player: &Transform,
    guard: &Transform,
    dummy: &Transform,
) {
    if !story.in_conversation() {
        if !placement.settled {
            return;
        }
        if near_guard(player, guard) {
            if keys.just_pressed(KeyCode::KeyE) {
                story.talk(None);
            } else if keys.just_pressed(KeyCode::KeyT)
                && let Some((topic, _)) = story.topic()
            {
                story.talk(Some(topic));
            }
        }
        // Walking away from the dummy gives the fight up.
        if story.busy() && !within(player, dummy, DISENGAGE_RANGE) {
            story.stop();
        }
        if within(player, dummy, STRIKE_RANGE) {
            if keys.just_pressed(KeyCode::KeyF) {
                story.strike();
            }
            if keys.just_pressed(KeyCode::KeyG) {
                story.power_strike();
            }
        }
        if keys.just_pressed(KeyCode::KeyV) {
            story.stop();
        }
        if keys.just_pressed(KeyCode::KeyH) {
            story.drink();
        }
        for (key, stat) in [
            (KeyCode::KeyZ, "strength"),
            (KeyCode::KeyX, "wisdom"),
            (KeyCode::KeyC, "vitality"),
        ] {
            if keys.just_pressed(key) && story.points().0 > 0 {
                story.spend(stat);
            }
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
    dummy: Single<&Transform, (With<Dummy>, Without<GateHinge>)>,
    mut hinge: Single<&mut Transform, With<GateHinge>>,
    mut panel_box: Single<&mut Visibility, With<PanelBox>>,
    mut texts: ParamSet<(
        Single<&mut Text, With<SpeakerText>>,
        Single<&mut Text, With<BodyText>>,
        Single<&mut Text, With<SummaryText>>,
        Single<&mut Text, With<BarkText>>,
    )>,
    mut shown: Local<Option<(u64, bool, bool)>>,
) {
    story.tick(time.delta_secs());
    suspended.hold("conversation", story.in_conversation());

    // An unlocked gate swings open; a reload that locks it again swings it shut.
    let target = if story.gate_locked() { 0.0 } else { 1.9 };
    let (current, _, _) = hinge.rotation.to_euler(EulerRot::YXZ);
    let step = (target - current).clamp(-1.0, 1.0) * (time.delta_secs() * 3.0).min(1.0);
    if step.abs() > 1e-4 {
        hinge.rotation = Quat::from_rotation_y(current + step);
    }

    // Text is rebuilt only after a command, a load, or crossing the talk range.
    let near = placement.settled && near_guard(&player, &guard);
    let sparring = placement.settled && within(&player, &dummy, STRIKE_RANGE);
    let current = (story.revision(), near, sparring);
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
        None if near => {
            let mut hint = format!("[E] Talk to {}", story.guard_name());
            if let Some((_, topic)) = story.topic() {
                hint.push_str(&format!("    [T] {topic}"));
            }
            (String::new(), hint)
        }
        None if sparring => (
            String::new(),
            "[F] Strike    [G] Power strike    [V] Stop".into(),
        ),
        None => (String::new(), String::new()),
    };
    // A conversation that just ended releases the player.
    suspended.hold("conversation", story.in_conversation());
    **panel_box = if body.is_empty() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    let mut summary = story.summary();
    summary.push_str("    [F5] Save  [F9] Load");
    summary.push('\n');
    summary.push_str(&story.character());
    summary.push_str("    [H] Potion");
    let (attribute, learning) = story.points();
    if attribute + learning > 0 {
        summary.push_str(&format!(
            "\nUnspent: {attribute} attribute [Z] Strength [X] Wisdom [C] Vitality, \
             {learning} learning (ask the guard about lessons)"
        ));
    }
    if sparring || story.busy() {
        summary.push('\n');
        summary.push_str(&story.fight());
    }
    if !story.notice.is_empty() {
        summary = format!("{}\n{summary}", story.notice);
    }
    texts.p0().0 = speaker;
    texts.p1().0 = body;
    if texts.p2().0 != summary {
        // The log follows the quest line; the sheet and the fight change many times a second.
        let headline = |text: &str| text.lines().next().unwrap_or_default().to_owned();
        if headline(&texts.p2().0) != headline(&summary) {
            info!("Story status: {}", headline(&summary));
        }
        texts.p2().0 = summary;
    }
    let bark = match story.bark() {
        Some(Bark { speaker, text }) => format!("{speaker}: {text}"),
        None => String::new(),
    };
    if texts.p3().0 != bark {
        if !bark.is_empty() {
            info!("Story bark: {bark}");
        }
        texts.p3().0 = bark;
    }
}
