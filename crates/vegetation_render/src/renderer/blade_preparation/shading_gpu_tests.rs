//! Opt-in shader A/B check with identical frozen instances, camera, wind and MSAA samples.
//!
//! `YARRA_GRASS_REFERENCE_SHADERS` mirrors `assets/shaders`: each `.wesl` file in it replaces the
//! shader at the same path, or joins them as a module of the reference's own. A reference for the
//! whole grass pipeline also regenerates placement and compares the instances.
use super::*;
use bevy::shader::Shader;

fn reference_files(root: &std::path::Path, directory: &std::path::Path, files: &mut Vec<String>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            reference_files(root, &path, files);
        } else if path.extension().is_some_and(|e| e == "wesl") {
            let relative = path.strip_prefix(root).unwrap();
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
#[ignore = "requires a native GPU and YARRA_GRASS_REFERENCE_SHADERS containing compatible baseline shaders"]
fn shading_matches_reference_in_frozen_scene() {
    let reference = std::path::PathBuf::from(
        std::env::var_os("YARRA_GRASS_REFERENCE_SHADERS")
            .expect("set the directory containing baseline shaders, laid out as assets/shaders"),
    );
    let mut app = test_app();
    settled_pixels(&mut app);
    let mut names = Vec::new();
    reference_files(&reference, &reference, &mut names);
    names.sort();
    let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    let mut handles: Vec<Handle<Shader>> = Vec::new();
    let mut current = Vec::new();
    let mut baseline = Vec::new();
    for name in &names {
        let path = format!("shaders/{name}");
        let source = Shader::from_wesl(
            std::fs::read_to_string(reference.join(name)).unwrap(),
            path.clone(),
        );
        if shaders.join(name).is_file() {
            let handle = app.world().resource::<AssetServer>().load(path);
            current.push(Some(
                app.world()
                    .resource::<Assets<Shader>>()
                    .get(&handle)
                    .unwrap_or_else(|| panic!("{name} is not loaded"))
                    .clone(),
            ));
            handles.push(handle);
        } else {
            // A module only the reference has; nothing current imports it.
            handles.push(
                app.world_mut()
                    .resource_mut::<Assets<Shader>>()
                    .add(source.clone()),
            );
            current.push(None);
        }
        baseline.push(Some(source));
    }
    let placement = ["vegetation/placement.wesl", "vegetation/schedule.wesl"]
        .iter()
        .all(|name| names.iter().any(|n| n == name));
    let install = |app: &mut App, sources: &[Option<Shader>]| {
        let mut assets = app.world_mut().resource_mut::<Assets<Shader>>();
        for (handle, source) in handles.iter().zip(sources) {
            if let Some(source) = source {
                assets.insert(handle.id(), source.clone()).unwrap();
            }
        }
    };
    enum DrawPath {
        Prepared,
        Fallback,
        Overflow,
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
    // Use the same shader A/B comparison for both ways the production low mesh can reach
    // procedural preparation. Overflow is last.
    .chain(
        [
            DrawPath::Lighting(VegetationLightingMode::UnlitDiagnostic),
            DrawPath::Fallback,
            DrawPath::Overflow,
        ]
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
                ..vegetation::CanopyShading::default_enabled()
            };
            world.resource_mut::<VegetationBladePreparation>().enabled =
                !matches!(path, DrawPath::Fallback);
            let mut settings = world.resource_mut::<VegetationSettings>();
            settings.profile_mode = VegetationProfileMode::Full;
            settings.lighting_mode = match path {
                DrawPath::Lighting(mode) => mode,
                _ => VegetationLightingMode::RoundedGloss,
            };
        }
        settled_pixels(&mut app);
        app.world_mut()
            .resource_mut::<VegetationSettings>()
            .profile_mode = VegetationProfileMode::DrawFrozen;
        let actual = settled_pixels(&mut app);
        let instances = generated_instances(&app);
        let stats = snapshot(&app);
        let counts = stats.emitted_instances;
        assert!(
            counts[3] > 100,
            "must exercise production low paired blades"
        );
        for (seen, count) in exercised_bins.iter_mut().zip(counts) {
            *seen |= count > 0;
        }
        exercised_morph |= instances.iter().flatten().any(|r| {
            let morph = (r[5] >> 16) & 0x7fff;
            r[5] >> 31 == 0 && morph > 0 && morph < 0x7fff
        });
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
        if placement {
            install(&mut app, &baseline);
            app.world_mut()
                .resource_mut::<VegetationSettings>()
                .profile_mode = VegetationProfileMode::Full;
            settled_pixels(&mut app);
            assert_eq!(
                generated_instances(&app),
                instances,
                "placement changed: case={case}"
            );
            install(&mut app, &current);
            settled_pixels(&mut app);
            app.world_mut()
                .resource_mut::<VegetationSettings>()
                .profile_mode = VegetationProfileMode::DrawFrozen;
        }
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
