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
    // Include the 48 m sheen cutoff, near canopy fade, thin and tall canopy envelopes,
    // and both MSAA settings. Generation is allowed only before each A/B pair.
    for (case, (position, msaa, enabled, height, near_strength)) in [
        (Vec3::new(12.0, 3.0, 18.0), Msaa::Off, false, 0.24, 0.0),
        (Vec3::new(12.0, 3.0, 18.0), Msaa::Sample4, true, 0.24, 0.0),
        (Vec3::new(12.0, 18.0, 8.5), Msaa::Sample4, true, 0.02, 1.0),
        (Vec3::new(12.0, 10.0, 55.0), Msaa::Sample4, true, 0.24, 0.0),
        (Vec3::new(12.0, 10.0, 75.0), Msaa::Sample4, true, 1.5, 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        install(&mut app, &current);
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
            world.resource_mut::<VegetationDebugSettings>().profile_mode =
                VegetationProfileMode::Full;
        }
        settled_pixels(&mut app);
        app.world_mut()
            .resource_mut::<VegetationDebugSettings>()
            .profile_mode = VegetationProfileMode::DrawFrozen;
        let actual = settled_pixels(&mut app);
        let counts = snapshot(&app).emitted_instances;
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
}
