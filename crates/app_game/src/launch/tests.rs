use super::*;
fn parse(args: &[&str]) -> Result<LaunchOptions, String> {
    LaunchOptions::parse(args.iter().map(OsString::from))
}
#[test]
fn defaults_and_paths_are_explicit() {
    let options = parse(&[]).unwrap();
    assert_eq!(options.fps, 0);
    assert!(!options.debug_world_switch);
    assert!(!options.counters);
    assert_eq!(options.upscaler, upscaling::UpscaleMethod::Auto);
    assert!(options.profile.is_none());
    let options = parse(&[
        "--world-db",
        "a path/world.sqlite",
        "--fps",
        "75",
        "--performance-open",
    ])
    .unwrap();
    assert_eq!(
        options.world_db.unwrap(),
        PathBuf::from("a path/world.sqlite")
    );
    assert_eq!(options.fps, 75);
    assert!(options.panel_open);
    assert!(!options.audit_log);
    let audit = parse(&["--render-audit"]).unwrap();
    assert!(audit.panel_open && audit.audit_log);
}
#[test]
fn malformed_and_retired_options_fail_before_startup() {
    for args in [
        vec!["--typo"],
        vec!["--fps"],
        vec!["--fps", "--performance-open"],
        vec!["--fps", "14"],
        vec!["--fps", "60", "--fps", "120"],
        vec!["--upscaler", "magic"],
        vec!["--grass-field-baseline"],
        vec!["--grass-bands", "off"],
        vec!["--vegetation-v2-debug"],
        vec!["--terrain-lod"],
        vec!["--terrain-prepared"],
        vec!["--frame-pacing-display-only"],
        vec!["--grass-prepared-blades", "1"],
        vec!["--gpu-timing-off", "--gpu-timing-detail"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}
#[test]
fn profile_and_repro_ownership_is_validated() {
    for args in [
        vec!["--profile-size", "game"],
        vec!["--render-frames", "1000"],
        vec!["--render-repro", "missing"],
        vec!["--render-repro", "landscape"],
        vec!["--profile-seconds", "10", "--fps", "60"],
        vec![
            "--profile-seconds",
            "10",
            "--render-repro",
            "grass-close",
            "--render-snapshot",
            "a.png",
        ],
        vec!["--streaming-smoke", "--profile-seconds", "10"],
        vec![
            "--render-repro",
            "grass-close",
            "--render-snapshot-frames",
            "500",
        ],
        vec!["--profile-seconds", "10", "--profile-temporal-bypass"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    assert!(
        parse(&[
            "--profile-seconds",
            "10",
            "--fps",
            "60",
            "--profile-native-pacing"
        ])
        .is_ok()
    );
}
#[test]
fn supported_automation_commands_keep_their_configuration() {
    let options = parse(&[
        "--world-db",
        "snapshot.sqlite",
        "--canopy-look",
        "canopy.ron",
        "--render-repro",
        "grass-soak",
        "--profile-seconds",
        "30",
        "--profile-warmup",
        "15",
        "--profile-size",
        "game",
        "--profile-fps",
        "60",
        "--profile-window",
        "fullscreen",
        "--profile-msaa",
        "4",
        "--profile-grass",
        "full",
        "--grass-density",
        "balanced",
        "--grass-prepared-blades",
        "262144",
    ])
    .unwrap();
    assert!(options.profile.is_some());
    assert_eq!(options.repro.unwrap().name, "grass-soak");
    assert_eq!(options.prepared_blades, Some(262144));
    assert!(options.audit_log);
    for name in crate::repro::NAMES {
        let mut args = vec![
            "--render-repro",
            name,
            "--render-frames",
            "1000",
            "--render-snapshot",
            "a.png",
            "--render-snapshot-frames",
            "600,900",
        ];
        if name.starts_with("landscape") {
            args.extend(["--start-view", "view.ron"]);
        }
        assert!(parse(&args).is_ok(), "{name}");
    }
}

#[test]
fn diagnostic_composition_is_explicit_and_rejects_incompatible_controls() {
    assert_eq!(parse(&[]).unwrap().diagnostics, DiagnosticsMode::Full);
    for (mode, expected) in [
        ("off", DiagnosticsMode::Off),
        ("panel", DiagnosticsMode::Panel),
        ("full", DiagnosticsMode::Full),
    ] {
        assert_eq!(
            parse(&["--diagnostics", mode]).unwrap().diagnostics,
            expected
        );
    }
    for args in [
        vec!["--diagnostics", "typo"],
        vec!["--diagnostics", "off", "--performance-open"],
        vec!["--diagnostics", "off", "--render-audit"],
        vec!["--diagnostics", "off", "--grass-counters"],
        vec!["--diagnostics", "off", "--metalfx-timing-log"],
        vec!["--diagnostics", "off", "--trace-camera-input"],
        vec!["--diagnostics", "off", "--timing-log"],
        vec!["--diagnostics", "panel", "--gpu-timing-detail"],
        vec!["--diagnostics", "panel", "--gpu-timing-off"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    assert!(parse(&["--diagnostics", "panel", "--performance-open"]).is_ok());
    // Finite repro and timed profile automation work independently of F1/probes.
    assert!(
        parse(&[
            "--diagnostics",
            "off",
            "--render-repro",
            "grass-close",
            "--render-frames",
            "900"
        ])
        .is_ok()
    );
    assert!(parse(&["--diagnostics", "off", "--profile-seconds", "10"]).is_ok());
}
#[test]
fn weather_defaults_to_automatic_play_and_authored_measurements() {
    assert_eq!(parse(&[]).unwrap().weather, engine::WeatherStart::Automatic);
    assert_eq!(
        parse(&["--weather", "Rain"]).unwrap().weather,
        engine::WeatherStart::Manual(engine::WeatherKind::Rain)
    );
    assert_eq!(
        parse(&["--weather", "authored"]).unwrap().weather,
        engine::WeatherStart::Authored
    );
    assert_eq!(
        parse(&["--render-repro", "grass-close"]).unwrap().weather,
        engine::WeatherStart::Authored
    );
    assert_eq!(
        parse(&["--profile-seconds", "10"]).unwrap().weather,
        engine::WeatherStart::Authored
    );
    assert_eq!(
        parse(&["--profile-seconds", "10", "--weather", "auto"])
            .unwrap()
            .weather,
        engine::WeatherStart::Automatic
    );
    assert!(parse(&["--weather", "snow"]).is_err());
    assert!(parse(&["--weather"]).is_err());
}
