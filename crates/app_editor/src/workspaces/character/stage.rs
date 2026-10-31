//! The stage the character stands on: an orbiting camera in the free part of the window, a neutral
//! floor and a choice of lights, all independent of the world's time and weather.

use bevy::gizmos::config::{DefaultGizmoConfigGroup, GizmoConfigStore};
use bevy::{
    camera::{Exposure, Viewport, visibility::RenderLayers},
    prelude::*,
};
use bevy_egui::egui;
use engine::{WorldSun, orbit_transform};

use super::{CharacterWorkspaceCamera, CompareActor, PreviewActor};
use crate::shell::EditorUiFrame;
use crate::workspaces::SavedDaylight;

/// The stage's layer: the world's terrain and objects stay off it.
const LAYER: usize = 29;

/// The compared character stands this far to the side, metres.
pub(super) const COMPARE_ASIDE: f32 = 0.85;

const FLOOR_RADIUS: f32 = 30.0;

const FLOOR_COLOR: Color = Color::srgb(0.32, 0.33, 0.35);

const BACKDROP: Color = Color::srgb(0.20, 0.21, 0.23);

const MIN_DISTANCE: f32 = 0.18;
const MAX_DISTANCE: f32 = 12.0;

/// Where the face is below the top of a character, and the camera's distance from it, metres.
const FACE_DROP: f32 = 0.13;

const FACE_DISTANCE: f32 = 0.62;

/// The fallback head top before a kit says otherwise, metres.
const DEFAULT_HEIGHT: f32 = 1.75;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Framing {
    #[default]
    Body,
    Face,
    ThreeQuarter,
    Profile,
}

impl Framing {
    const ALL: [Self; 4] = [Self::Body, Self::Face, Self::ThreeQuarter, Self::Profile];

    fn label(self) -> &'static str {
        match self {
            Self::Body => "Body",
            Self::Face => "Face",
            Self::ThreeQuarter => "¾",
            Self::Profile => "Profile",
        }
    }
}

/// One light: its direction by turn round the character from its front and rise, degrees; its
/// illuminance in lux and its colour.
#[derive(Clone, Copy)]
struct StageLight {
    turn: f32,
    rise: f32,
    lux: f32,
    color: [f32; 3],
}

impl StageLight {
    fn transform(&self, rig_turn: f32) -> Transform {
        let (turn, rise) = ((self.turn + rig_turn).to_radians(), self.rise.to_radians());
        let toward = Vec3::new(turn.sin() * rise.cos(), rise.sin(), turn.cos() * rise.cos());
        Transform::from_translation(toward * 30.0).looking_at(Vec3::ZERO, Vec3::Y)
    }

    fn color(&self) -> Color {
        Color::srgb(self.color[0], self.color[1], self.color[2])
    }
}

const fn light(turn: f32, rise: f32, lux: f32, color: [f32; 3]) -> StageLight {
    StageLight {
        turn,
        rise,
        lux,
        color,
    }
}

/// A set of lights. The key is the world's sun, so it casts the shadows; fill and rim add to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LightRig {
    #[default]
    Studio,
    Daylight,
    Overcast,
    Dusk,
}

struct RigLights {
    key: StageLight,
    fill: StageLight,
    rim: StageLight,
    ambient: (Color, f32),
    ev100: f32,
}

impl LightRig {
    const ALL: [Self; 4] = [Self::Studio, Self::Daylight, Self::Overcast, Self::Dusk];

    fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Daylight => "Daylight",
            Self::Overcast => "Overcast",
            Self::Dusk => "Dusk",
        }
    }

    fn lights(self) -> RigLights {
        match self {
            Self::Studio => RigLights {
                key: light(40.0, 35.0, 32_000.0, [1.0, 0.96, 0.91]),
                // From low down: it stands for the light the floor throws back under the chin.
                fill: light(-45.0, -22.0, 12_000.0, [0.92, 0.93, 1.0]),
                rim: light(165.0, 35.0, 14_000.0, [1.0, 1.0, 1.0]),
                ambient: (Color::srgb(0.80, 0.84, 0.92), 1_200.0),
                ev100: 13.5,
            },
            Self::Daylight => RigLights {
                key: light(60.0, 55.0, 90_000.0, [1.0, 0.97, 0.90]),
                fill: light(0.0, 0.0, 0.0, [1.0, 1.0, 1.0]),
                rim: light(0.0, 0.0, 0.0, [1.0, 1.0, 1.0]),
                ambient: (Color::srgb(0.62, 0.75, 1.0), 9_000.0),
                ev100: 15.0,
            },
            Self::Overcast => RigLights {
                key: light(10.0, 70.0, 14_000.0, [0.95, 0.97, 1.0]),
                fill: light(0.0, 0.0, 0.0, [1.0, 1.0, 1.0]),
                rim: light(0.0, 0.0, 0.0, [1.0, 1.0, 1.0]),
                ambient: (Color::srgb(0.90, 0.93, 1.0), 6_000.0),
                ev100: 13.5,
            },
            Self::Dusk => RigLights {
                key: light(75.0, 7.0, 14_000.0, [1.0, 0.62, 0.34]),
                fill: light(-70.0, 15.0, 800.0, [0.55, 0.65, 1.0]),
                rim: light(0.0, 0.0, 0.0, [1.0, 1.0, 1.0]),
                ambient: (Color::srgb(0.50, 0.60, 1.0), 900.0),
                ev100: 12.0,
            },
        }
    }
}

#[derive(Resource)]
pub(super) struct Stage {
    yaw: f32,
    pitch: f32,
    distance: f32,
    focus: Vec3,
    pub(super) height: f32,
    rig: LightRig,
    rig_turn: f32,
    exposure_offset: f32,
    floor: bool,
    grid: bool,
    framed: Option<Framing>,
    framing: Framing,
    /// The free part of the window, physical pixels; the panels fill the rest.
    viewport: Option<(UVec2, UVec2)>,
    pub(super) comparing: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 5.0,
            distance: 3.4,
            focus: Vec3::new(0.0, 0.95, 0.0),
            height: DEFAULT_HEIGHT,
            rig: LightRig::default(),
            rig_turn: 0.0,
            exposure_offset: 0.0,
            floor: true,
            grid: false,
            framed: None,
            framing: Framing::Body,
            viewport: None,
            comparing: false,
        }
    }
}

impl Stage {
    fn frame(&mut self) {
        let face = Vec3::new(0.0, self.height - FACE_DROP, 0.0);
        let (yaw, pitch, distance, focus) = match self.framing {
            Framing::Body if self.comparing => {
                (0.0, 5.0, 4.2, Vec3::new(-COMPARE_ASIDE / 2.0, 0.95, 0.0))
            }
            Framing::Body => (0.0, 5.0, 3.4, Vec3::new(0.0, 0.95, 0.0)),
            Framing::Face => (0.0, 2.0, FACE_DISTANCE, face),
            Framing::ThreeQuarter => (35.0, 2.0, FACE_DISTANCE, face),
            Framing::Profile => (90.0, 2.0, FACE_DISTANCE, face),
        };
        self.yaw = yaw;
        self.pitch = pitch;
        self.distance = distance;
        self.focus = focus;
        self.framed = Some(self.framing);
    }

    fn view(&self) -> Transform {
        orbit_transform(
            self.focus,
            self.yaw.to_radians(),
            self.pitch.to_radians(),
            self.distance,
        )
    }

    /// Orbits; pitch stops short of the poles.
    fn orbit(&mut self, right: f32, down: f32) {
        self.yaw = (self.yaw - right * 0.4).rem_euclid(360.0);
        self.pitch = (self.pitch + down * 0.3).clamp(-60.0, 80.0);
    }

    /// Slides the focus in the view's plane, by as much as the pointer moves over the subject.
    fn pan(&mut self, right: f32, down: f32, viewport_height: f32) {
        let view = self.view();
        let per_point = self.distance * 0.9 / viewport_height.max(1.0);
        self.focus += (view.up() * down - view.right() * right) * per_point;
    }

    fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.002).exp()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    #[cfg(test)]
    pub(super) fn set_framing_for_test(&mut self, framing: Framing) -> Transform {
        self.framing = framing;
        self.frame();
        self.view()
    }
}

#[derive(Component)]
pub(super) struct StageFloor;

#[derive(Component, Clone, Copy)]
pub(super) enum StageFill {
    Fill,
    Rim,
}

pub(super) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Circle::new(FLOOR_RADIUS))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: FLOOR_COLOR,
            perceptual_roughness: 0.92,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        Visibility::Hidden,
        RenderLayers::layer(LAYER),
        StageFloor,
        Name::new("Character stage floor"),
    ));

    for (light, name) in [
        (StageFill::Fill, "Character stage fill"),
        (StageFill::Rim, "Character stage rim"),
    ] {
        commands.spawn((
            // With shadows, or they would shine through the head onto the insides of the ears
            // and lips.
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::default(),
            Visibility::Hidden,
            RenderLayers::layer(LAYER),
            light,
            Name::new(name),
        ));
    }
}

pub(super) fn camera_bundle() -> impl Bundle {
    (
        Camera {
            is_active: false,
            clear_color: ClearColorConfig::Custom(BACKDROP),
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: 30f32.to_radians(),
            near: 0.02,
            far: 200.0,
            ..default()
        }),
        RenderLayers::layer(LAYER),
        Stage::default().view(),
    )
}

/// The world's sun, as the workspace finds it.
type SunToSave<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static DirectionalLight,
        Option<&'static RenderLayers>,
    ),
    With<WorldSun>,
>;

/// The characters on the stage.
type StageActors<'w, 's> = Query<'w, 's, Entity, Or<(With<PreviewActor>, With<CompareActor>)>>;

/// What shows only while the workspace is open.
type StageShown<'w, 's> =
    Query<'w, 's, &'static mut Visibility, Or<(With<StageFloor>, With<StageFill>)>>;

/// The fill and rim lights, kept apart from the sun and camera for their transforms.
type FillLights<'w, 's> = Query<
    'w,
    's,
    (
        &'static StageFill,
        &'static mut Transform,
        &'static mut DirectionalLight,
    ),
    (Without<WorldSun>, Without<CharacterWorkspaceCamera>),
>;

/// The camera, kept apart from the lights for its transform.
type StageCamera<'w, 's> = Single<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Camera,
        &'static mut Exposure,
    ),
    (
        With<CharacterWorkspaceCamera>,
        Without<WorldSun>,
        Without<StageFill>,
    ),
>;

#[derive(Resource, Default)]
pub(super) struct SavedLight(Option<(SavedDaylight, Option<RenderLayers>)>);

/// A character's meshes appear as its scene loads; each joins the stage's layer as it does.
pub(super) fn gather_meshes(
    mut commands: Commands,
    actors: StageActors,
    children: Query<&Children>,
    loose: Query<(), (With<Mesh3d>, Without<RenderLayers>)>,
) {
    for actor in &actors {
        for entity in children.iter_descendants(actor) {
            if loose.contains(entity) {
                commands.entity(entity).insert(RenderLayers::layer(LAYER));
            }
        }
    }
}

pub(super) fn enter(
    mut commands: Commands,
    mut saved: ResMut<SavedLight>,
    sun: SunToSave,
    ambient: Option<Res<GlobalAmbientLight>>,
    mut shown: StageShown,
    gizmos: Option<ResMut<GizmoConfigStore>>,
) {
    if let (Ok((entity, transform, light, layers)), Some(ambient)) = (sun.single(), ambient) {
        saved.0 = Some((
            SavedDaylight::save(transform, light, &ambient),
            layers.cloned(),
        ));
        commands.entity(entity).insert(RenderLayers::layer(LAYER));
    }

    for mut visibility in &mut shown {
        *visibility = Visibility::Inherited;
    }

    if let Some(mut gizmos) = gizmos {
        gizmos
            .config_mut::<DefaultGizmoConfigGroup>()
            .0
            .render_layers = RenderLayers::layer(LAYER);
    }
}

pub(super) fn exit(
    mut commands: Commands,
    mut saved: ResMut<SavedLight>,
    mut sun: Query<(Entity, &mut Transform, &mut DirectionalLight), With<WorldSun>>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    mut shown: StageShown,
    gizmos: Option<ResMut<GizmoConfigStore>>,
) {
    if let (Some((saved, layers)), Ok((entity, mut transform, mut light)), Some(mut ambient)) =
        (saved.0.take(), sun.single_mut(), ambient)
    {
        saved.restore(&mut transform, &mut light, &mut ambient);

        match layers {
            Some(layers) => commands.entity(entity).insert(layers),
            None => commands.entity(entity).remove::<RenderLayers>(),
        };
    }

    for mut visibility in &mut shown {
        *visibility = Visibility::Hidden;
    }

    if let Some(mut gizmos) = gizmos {
        gizmos
            .config_mut::<DefaultGizmoConfigGroup>()
            .0
            .render_layers = RenderLayers::default();
    }
}

/// Holds the sun, the ambient light and the camera to the stage: the world's clock moves the sun
/// otherwise.
pub(super) fn apply(
    mut stage: ResMut<Stage>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<WorldSun>>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    mut fills: FillLights,
    mut camera: StageCamera,
    mut floor: Query<&mut Visibility, (With<StageFloor>, Without<CompareActor>)>,
) {
    if stage.framed != Some(stage.framing) {
        stage.frame();
    }

    let rig = stage.rig.lights();

    if let Ok((mut transform, mut light)) = sun.single_mut() {
        *transform = rig.key.transform(stage.rig_turn);
        light.color = rig.key.color();
        light.illuminance = rig.key.lux;
    }

    if let Some(mut ambient) = ambient {
        ambient.color = rig.ambient.0;
        ambient.brightness = rig.ambient.1;
    }

    for (kind, mut transform, mut light) in &mut fills {
        let spec = match kind {
            StageFill::Fill => rig.fill,
            StageFill::Rim => rig.rim,
        };
        *transform = spec.transform(stage.rig_turn);
        light.color = spec.color();
        light.illuminance = spec.lux;
    }

    let (transform, view, exposure) = &mut *camera;
    **transform = stage.view();
    exposure.ev100 = rig.ev100 + stage.exposure_offset;
    view.viewport = stage.viewport.map(|(position, size)| Viewport {
        physical_position: position,
        physical_size: size,
        ..default()
    });

    for mut visibility in &mut floor {
        *visibility = if stage.floor {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

pub(super) fn draw_grid(stage: Res<Stage>, mut gizmos: Gizmos) {
    if !stage.grid {
        return;
    }

    let color = Color::srgba(0.75, 0.78, 0.82, 0.35);

    for coordinate in -10..=10 {
        let coordinate = coordinate as f32 * 0.5;
        let along = 5.0;
        gizmos.line(
            Vec3::new(coordinate, 0.002, -along),
            Vec3::new(coordinate, 0.002, along),
            color,
        );
        gizmos.line(
            Vec3::new(-along, 0.002, coordinate),
            Vec3::new(along, 0.002, coordinate),
            color,
        );
    }
}

/// The free part of the window: where the camera draws, the toolbar over it and the mouse's orbit
/// (left drag), pan (right or middle drag) and zoom (scroll).
pub(super) fn viewport_panel(mut frame: ResMut<EditorUiFrame>, mut stage: ResMut<Stage>) {
    let Some(workspace_ui) = frame.0.as_mut() else {
        return;
    };

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(workspace_ui, |ui| {
            let rect = ui.max_rect();
            let scale = ui.ctx().pixels_per_point();
            let size = (rect.size() * scale).max(egui::vec2(1.0, 1.0));
            stage.viewport = Some((
                UVec2::new(
                    (rect.min.x * scale).round() as u32,
                    (rect.min.y * scale).round() as u32,
                ),
                UVec2::new(size.x.round() as u32, size.y.round() as u32),
            ));
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            mouse(ui, &response, &mut stage, rect.height());
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(8.0)), |ui| {
                toolbar(ui, &mut stage)
            });
        });
}

fn mouse(ui: &egui::Ui, response: &egui::Response, stage: &mut Stage, height: f32) {
    let delta = ui.input(|input| input.pointer.delta());

    if response.dragged_by(egui::PointerButton::Primary) {
        stage.orbit(delta.x, delta.y);
    }

    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        stage.pan(delta.x, delta.y, height);
    }

    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);

        if scroll != 0.0 {
            stage.zoom(scroll);
        }
    }

    if response.double_clicked() {
        stage.framed = None;
    }
}

fn toolbar(ui: &mut egui::Ui, stage: &mut Stage) {
    ui.horizontal_wrapped(|ui| {
        for framing in Framing::ALL {
            if ui
                .selectable_label(stage.framing == framing, framing.label())
                .clicked()
            {
                stage.framing = framing;
                stage.framed = None;
            }
        }

        ui.separator();
        egui::ComboBox::from_id_salt("stage_rig")
            .selected_text(stage.rig.label())
            .width(90.0)
            .show_ui(ui, |ui| {
                for rig in LightRig::ALL {
                    ui.selectable_value(&mut stage.rig, rig, rig.label());
                }
            });
        ui.add(
            egui::Slider::new(&mut stage.rig_turn, -180.0..=180.0)
                .text("turn")
                .fixed_decimals(0)
                .suffix("°"),
        );
        ui.add(
            egui::Slider::new(&mut stage.exposure_offset, -3.0..=3.0)
                .text("EV")
                .fixed_decimals(1),
        );
        ui.checkbox(&mut stage.floor, "Floor");
        ui.checkbox(&mut stage.grid, "Grid");
    });
}

#[cfg(test)]
pub(super) fn framing_faces(framing: Framing) -> Transform {
    let mut stage = Stage::default();
    stage.set_framing_for_test(framing)
}

#[cfg(test)]
pub(super) const FACE: Framing = Framing::Face;

#[cfg(test)]
pub(super) const PROFILE: Framing = Framing::Profile;
