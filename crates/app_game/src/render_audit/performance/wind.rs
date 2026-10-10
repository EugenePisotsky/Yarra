//! Session wind experiments on the native F1 panel.
use super::capture::CaptureSession;
use crate::render_audit::font;
use bevy::{
    prelude::*,
    ui::{Pressed, RelativeCursorPosition},
    ui_widgets::Button,
};
use engine::TreeWindTuning;
use vegetation_render::VegetationWind;

#[derive(Component, Clone, Copy)]
enum Knob {
    Strength,
    Gusts,
    Heading,
    Sway,
    Branches,
    Flutter,
    Rhythm,
}
impl Knob {
    fn info(self) -> (&'static str, f32, f32) {
        match self {
            Self::Strength => ("Wind strength", 0., 2.),
            Self::Gusts => ("Gustiness", 0., 1.),
            Self::Heading => ("Direction (degrees)", -180., 180.),
            Self::Sway => ("Trunk bend", 0., 3.),
            Self::Branches => ("Branch movement", 0., 3.),
            Self::Flutter => ("Leaf / needle flutter", 0., 3.),
            Self::Rhythm => ("Trunk response period", 0.4, 2.),
        }
    }
    fn value(self, t: &TreeWindTuning) -> f32 {
        match self {
            Self::Strength => t.strength,
            Self::Gusts => t.gustiness,
            Self::Heading => t.heading,
            Self::Sway => t.sway,
            Self::Branches => t.branches,
            Self::Flutter => t.flutter,
            Self::Rhythm => t.rhythm,
        }
    }
    fn set(self, t: &mut TreeWindTuning, value: f32) {
        match self {
            Self::Strength => {
                t.strength = value;
                t.manual = true;
            }
            Self::Gusts => {
                t.gustiness = value;
                t.manual = true;
            }
            Self::Heading => {
                t.heading = value;
                t.manual = true;
            }
            Self::Sway => t.sway = value,
            Self::Branches => t.branches = value,
            Self::Flutter => t.flutter = value,
            Self::Rhythm => t.rhythm = value,
        }
    }
}
#[derive(Component)]
struct Track(Knob);
#[derive(Component)]
struct Fill(Knob);
#[derive(Component)]
struct Label(Knob);
#[derive(Component)]
struct Status;
#[derive(Component, Clone, Copy)]
enum Action {
    Weather,
    Calm,
    Breeze,
    Gusty,
    Strong,
    Reset,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(Update, (actions, drag).chain())
        .add_systems(PostUpdate, refresh.after(engine::TreeWindSystems));
}
pub(super) fn spawn(page: &mut ChildSpawnerCommands, button: impl Fn() -> (Node, BackgroundColor)) {
    page.spawn((Text::new("Drag a bar to tune. Strength, gusts and direction switch to manual shared wind; Follow weather restores automatic wind. Tree response controls apply to the new connected assets. Values are artistic tuning, not measured wind speeds."),font(12.)));
    page.spawn((Text::new(""), font(13.), Status));
    page.spawn(Node {
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(6),
        row_gap: px(6),
        ..default()
    })
    .with_children(|row| {
        for (a, label) in [
            (Action::Weather, "Follow weather"),
            (Action::Calm, "Calm"),
            (Action::Breeze, "Breeze"),
            (Action::Gusty, "Gusty"),
            (Action::Strong, "Strong"),
            (Action::Reset, "Reset wind tuning"),
        ] {
            let (node, color) = button();
            row.spawn((Button, a, node, color))
                .with_child((Text::new(label), font(13.)));
        }
    });
    for knob in [
        Knob::Strength,
        Knob::Gusts,
        Knob::Heading,
        Knob::Sway,
        Knob::Branches,
        Knob::Flutter,
        Knob::Rhythm,
    ] {
        page.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            ..default()
        })
        .with_children(|row| {
            row.spawn((Text::new(""), font(13.), Label(knob)));
            row.spawn((
                Button,
                Track(knob),
                RelativeCursorPosition::default(),
                Node {
                    height: px(24),
                    width: percent(100),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.10, 0.17, 0.22)),
            ))
            .with_child((
                Fill(knob),
                Node {
                    height: percent(100),
                    width: percent(0),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.20, 0.65, 0.55)),
                bevy::ui::FocusPolicy::Pass,
            ));
        });
    }
}
fn actions(
    clicks: Query<&Action, Added<Pressed>>,
    tuning: Option<ResMut<TreeWindTuning>>,
    wind: Option<Res<VegetationWind>>,
    session: Res<CaptureSession>,
) {
    let Some(mut t) = tuning else {
        return;
    };
    if session.recording() {
        return;
    }
    for action in &clicks {
        match action {
            Action::Weather => t.manual = false,
            Action::Reset => *t = TreeWindTuning::default(),
            _ => {
                begin_manual(&mut t, wind.as_deref());
                t.manual = true;
                (t.strength, t.gustiness) = match action {
                    Action::Calm => (0., 0.),
                    Action::Breeze => (0.45, 0.35),
                    Action::Gusty => (1.0, 0.95),
                    Action::Strong => (1.7, 1.),
                    _ => unreachable!(),
                };
            }
        }
    }
}
fn drag(
    tracks: Query<(Has<Pressed>, &RelativeCursorPosition, &Track)>,
    tuning: Option<ResMut<TreeWindTuning>>,
    wind: Option<Res<VegetationWind>>,
    session: Res<CaptureSession>,
) {
    let Some(mut t) = tuning else {
        return;
    };
    if session.recording() {
        return;
    }
    for (pressed, cursor, track) in &tracks {
        if pressed && let Some(p) = cursor.normalized {
            let (_, lo, hi) = track.0.info();
            if matches!(track.0, Knob::Strength | Knob::Gusts | Knob::Heading) {
                begin_manual(&mut t, wind.as_deref());
            }
            track.0.set(&mut t, lo + p.x.clamp(0., 1.) * (hi - lo));
        }
    }
}
fn refresh(
    tuning: Option<Res<TreeWindTuning>>,
    wind: Option<Res<VegetationWind>>,
    mut texts: Query<(&Label, &mut Text), Without<Status>>,
    mut fills: Query<(&Fill, &mut Node)>,
    mut status: Query<&mut Text, With<Status>>,
) {
    let Some(t) = tuning else {
        return;
    };
    let mut display = t.clone();
    if !display.manual {
        begin_manual(&mut display, wind.as_deref());
    }
    for (label, mut text) in &mut texts {
        let (name, _, _) = label.0.info();
        let value = format!("{name}: {:.2}", label.0.value(&display));
        if text.0 != value {
            **text = value;
        }
    }
    for (fill, mut node) in &mut fills {
        let (_, lo, hi) = fill.0.info();
        let width = percent(100. * ((fill.0.value(&display) - lo) / (hi - lo)).clamp(0., 1.));
        if node.width != width {
            node.width = width;
        }
    }
    for mut text in &mut status {
        let value = format!(
            "{} | effective strength {:.2}{}",
            if t.manual {
                "Manual wind"
            } else {
                "Following weather"
            },
            wind.as_ref().map_or(0., |w| w.strength),
            if wind.as_ref().is_some_and(|w| !w.enabled) {
                " | wind disabled in Features"
            } else {
                ""
            }
        );
        if text.0 != value {
            **text = value;
        }
    }
}

fn begin_manual(t: &mut TreeWindTuning, wind: Option<&VegetationWind>) {
    if !t.manual
        && let Some(wind) = wind
    {
        t.strength = wind.strength;
        t.gustiness = wind.gustiness;
        t.heading = wind.direction.y.atan2(wind.direction.x).to_degrees();
    }
}
