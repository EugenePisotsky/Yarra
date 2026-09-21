use super::*;

#[test]
fn selection_distinguishes_requested_backend_from_fallback() {
    let mut caps = UpscalingCapabilities {
        backends: vec![BackendCapability {
            method: UpscaleMethod::MetalFxSpatial,
            supported: false,
            reason: Some("Unavailable on this device".into()),
            requirements: SPATIAL_REQUIREMENTS,
        }],
    };
    for request in [UpscaleMethod::Auto, UpscaleMethod::MetalFxSpatial] {
        assert_eq!(
            caps.select(request),
            (
                UpscaleMethod::Linear,
                Some("Unavailable on this device".into())
            )
        );
    }
    caps.backends[0].supported = true;
    caps.backends[0].reason = None;
    assert_eq!(
        caps.select(UpscaleMethod::Auto),
        (UpscaleMethod::MetalFxSpatial, None)
    );
    assert_eq!(
        caps.select(UpscaleMethod::Linear),
        (UpscaleMethod::Linear, None)
    );
}

#[test]
fn old_backend_results_do_not_replace_new_request_status() {
    let mut app = App::new();
    app.add_plugins(UpscalingPlugin);
    let entity = app
        .world_mut()
        .spawn(UpscaleView {
            input: default(),
            output: default(),
            method: UpscaleMethod::Linear,
            enabled: true,
        })
        .id();
    app.world()
        .resource::<Bridge>()
        .0
        .lock()
        .unwrap()
        .statuses
        .insert(
            entity,
            UpscaleStatus {
                requested: UpscaleMethod::Auto,
                active: Some(UpscaleMethod::MetalFxSpatial),
                ..default()
            },
        );
    app.update();
    assert_eq!(
        app.world().get::<UpscaleStatus>(entity).unwrap().active,
        None
    );
}
