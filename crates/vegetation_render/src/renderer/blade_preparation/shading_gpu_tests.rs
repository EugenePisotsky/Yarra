//! Opt-in shader A/B check with identical frozen instances, camera, wind and MSAA samples.
use super::*;
use bevy::shader::Shader;

#[test]
#[ignore = "requires a native GPU and YARRA_GRASS_REFERENCE_SHADERS containing compatible baseline shaders"]
fn shading_matches_reference_in_frozen_scene() {
    let reference = std::path::PathBuf::from(
        std::env::var_os("YARRA_GRASS_REFERENCE_SHADERS")
            .expect("set the directory containing baseline draw and canopy shaders"),
    );
    let mut app = test_app();
    settled_pixels(&mut app);
    let names = ["vegetation_debug_draw.wgsl", "grass_canopy.wgsl"];
    let handles: Vec<Handle<Shader>> = names
        .iter()
        .map(|name| {
            app.world()
                .resource::<AssetServer>()
                .load(format!("shaders/{name}"))
        })
        .collect();
    let current: Vec<_> = handles
        .iter()
        .map(|handle| {
            app.world()
                .resource::<Assets<Shader>>()
                .get(handle)
                .unwrap()
                .clone()
        })
        .collect();
    let baseline: Vec<_> = names
        .iter()
        .map(|name| {
            Shader::from_wgsl(
                std::fs::read_to_string(reference.join(name)).unwrap(),
                format!("shaders/{name}"),
            )
        })
        .collect();
    let install = |app: &mut App, sources: &[Shader]| {
        let mut assets = app.world_mut().resource_mut::<Assets<Shader>>();
        for (handle, source) in handles.iter().zip(sources) {
            assets.insert(handle.id(), source.clone()).unwrap();
        }
    };
    enum DrawPath {
        Prepared,
        Fallback,
        Overflow,
        Inspection(VegetationShapeInspection),
        Bands,
        Lighting(VegetationLightingMode),
    }
    let mut exercised_bins = [false; 4];
    let mut exercised_morph = false;
    // Include the 48 m sheen cutoff, near canopy fade, thin and tall canopy envelopes,
    // and both MSAA settings. Generation is allowed only before each A/B pair.
    for (case, (position, msaa, enabled, height, near_strength, path)) in [
        (Vec3::new(12.0, 3.0, 18.0), Msaa::Off, false, 0.24, 0.0),
        (Vec3::new(12.0, 3.0, 18.0), Msaa::Sample4, true, 0.24, 0.0),
        (Vec3::new(12.0, 18.0, 8.5), Msaa::Sample4, true, 0.02, 1.0),
        (Vec3::new(12.0, 10.0, 55.0), Msaa::Sample4, true, 0.24, 0.0),
        (Vec3::new(12.0, 10.0, 75.0), Msaa::Sample4, true, 1.5, 1.0),
    ]
    .into_iter()
    .map(|(position, msaa, enabled, height, near_strength)| {
        (
            position,
            msaa,
            enabled,
            height,
            near_strength,
            DrawPath::Prepared,
        )
    })
    // Use the same shader A/B comparison for promoted inspection geometry and both
    // ways the production low mesh can reach procedural preparation. Overflow is last.
    .chain(
        VegetationShapeInspection::ALL
            .into_iter()
            .skip(1)
            .map(DrawPath::Inspection)
            .chain([
                DrawPath::Bands,
                DrawPath::Lighting(VegetationLightingMode::Legacy),
                DrawPath::Lighting(VegetationLightingMode::UnlitDiagnostic),
                DrawPath::Fallback,
                DrawPath::Overflow,
            ])
            .map(|path| {
                (
                    Vec3::new(12.0, 3.0, 18.0),
                    Msaa::Sample4,
                    true,
                    0.24,
                    0.0,
                    path,
                )
            }),
    )
    .enumerate()
    {
        install(&mut app, &current);
        if matches!(path, DrawPath::Overflow) {
            replace_arena(&mut app, 3);
        }
        {
            let world = app.world_mut();
            let mut cameras = world.query_filtered::<(&mut Msaa, &mut Transform), With<Camera3d>>();
            for (mut samples, mut transform) in cameras.iter_mut(world) {
                *samples = msaa;
                *transform = Transform::from_translation(position)
                    .looking_at(Vec3::new(12.0, 0.0, 8.0), Vec3::Y);
            }
            world.resource_mut::<VegetationWind>().phase_seconds = 1.73;
            world.resource_mut::<VegetationLighting>().canopy = vegetation::CanopyShading {
                enabled,
                height_metres: height,
                near_strength,
                distance_start: 15.8,
                distance_end: 20.0,
                ..vegetation::CanopyShading::experiment()
            };
            world.resource_mut::<VegetationBladePreparation>().enabled =
                !matches!(path, DrawPath::Fallback);
            let mut settings = world.resource_mut::<VegetationDebugSettings>();
            settings.profile_mode = VegetationProfileMode::Full;
            settings.shape_inspection = match path {
                DrawPath::Inspection(mode) => mode,
                _ => VegetationShapeInspection::Off,
            };
            settings.blade_bands = if matches!(path, DrawPath::Bands) {
                VegetationBladeBands::Medium
            } else {
                VegetationBladeBands::Off
            };
            settings.lighting_mode = match path {
                DrawPath::Lighting(mode) => mode,
                _ => VegetationLightingMode::RoundedGloss,
            };
        }
        settled_pixels(&mut app);
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .profile_mode = VegetationProfileMode::DrawFrozen;
        let actual = settled_pixels(&mut app);
        let stats = snapshot(&app);
        let counts = stats.emitted_instances;
        if !matches!(path, DrawPath::Inspection(_)) {
            assert!(
                counts[3] > 100,
                "must exercise production low paired blades"
            );
            for (seen, count) in exercised_bins.iter_mut().zip(counts) {
                *seen |= count > 0;
            }
            exercised_morph |= generated_instances(&app).iter().flatten().any(|r| {
                let morph = (r[5] >> 16) & 0x7fff;
                r[5] >> 31 == 0 && morph > 0 && morph < 0x7fff
            });
        }
        match path {
            DrawPath::Fallback => assert!(!stats.blade_preparation_enabled),
            DrawPath::Overflow => {
                assert!(stats.prepared_blades <= 3 && stats.preparation_fallback_blades > 100);
            }
            _ => assert!(stats.prepared_blades > 0),
        }
        assert!(
            counts.iter().sum::<u32>() > 100,
            "empty fixture: {counts:?}"
        );
        install(&mut app, &baseline);
        let expected = settled_pixels(&mut app);
        assert_eq!(snapshot(&app).emitted_instances, counts);
        let maximum = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        let mean = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / actual.len() as f64;
        eprintln!(
            "frozen shading case {case}: max byte error={maximum}, mean={mean:.6}, instances={counts:?}"
        );
        assert!(
            maximum <= 2 && mean < 0.01,
            "shading changed: case={case}, max={maximum}, mean={mean}"
        );
        // A second current render also checks that both shader swaps actually settle.
        install(&mut app, &current);
        assert_eq!(
            settled_pixels(&mut app),
            actual,
            "unstable frozen scene: case={case}"
        );
        if case == 1 {
            // Ensure this fixture exercises visible canopy shading, rather than comparing
            // two images whose boundary field or material never became active.
            app.world_mut()
                .resource_mut::<VegetationLighting>()
                .canopy
                .strength = 0.0;
            assert_ne!(
                settled_pixels(&mut app),
                actual,
                "canopy fixture has no visible effect"
            );
        }
    }
    assert!(
        exercised_bins.into_iter().all(|seen| seen),
        "cover single and paired, high and low bins"
    );
    assert!(exercised_morph, "cover partially morphed high geometry");
}
