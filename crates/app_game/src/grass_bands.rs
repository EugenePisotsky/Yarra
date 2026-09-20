//! Desktop playtest controls for the shared blade shading experiment.
use bevy::prelude::*;
use vegetation_render::{VegetationBladeBands, VegetationDebugSettings};

pub(super) fn install(app: &mut App) {
    let mut args = std::env::args();
    let mode = if args.any(|arg| arg == "--grass-bands") {
        match args.next().as_deref() {
            Some("off") => VegetationBladeBands::Off,
            Some("subtle") => VegetationBladeBands::Subtle,
            Some("medium") => VegetationBladeBands::Medium,
            _ => panic!("--grass-bands requires off, subtle, or medium"),
        }
    } else {
        VegetationBladeBands::Off
    };
    app.world_mut()
        .resource_mut::<VegetationDebugSettings>()
        .blade_bands = mode;
    warn!(
        "Grass blade shadow experiment: {} (B cycles Off/Subtle/Medium)",
        mode.label()
    );
    if !cfg!(target_os = "ios") {
        app.add_systems(Startup, setup).add_systems(Update, update);
    }
}

#[derive(Component)]
struct GrassBandStatus;

fn label(mode: VegetationBladeBands) -> String {
    format!(
        "Grass shadow marks: {} | B: Off / Subtle / Medium",
        mode.label(),
    )
}

fn setup(mut commands: Commands, settings: Res<VegetationDebugSettings>) {
    commands.spawn((
        Text::new(label(settings.blade_bands)),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.025, 0.03, 0.04, 0.75)),
        Node {
            position_type: PositionType::Absolute,
            right: px(14),
            bottom: px(14),
            padding: UiRect::all(px(6)),
            ..default()
        },
        GrassBandStatus,
        engine::DiagnosticOverlay,
    ));
}

fn update(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut status: Single<&mut Text, With<GrassBandStatus>>,
) {
    if keys.just_pressed(KeyCode::KeyB) {
        settings.blade_bands = match settings.blade_bands {
            VegetationBladeBands::Off => VegetationBladeBands::Subtle,
            VegetationBladeBands::Subtle => VegetationBladeBands::Medium,
            _ => VegetationBladeBands::Off,
        };
        warn!(
            "Grass blade shadow experiment: {}",
            settings.blade_bands.label()
        );
    }
    if settings.is_changed() {
        **status = Text::new(label(settings.blade_bands));
    }
}
