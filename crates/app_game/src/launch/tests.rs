use super::*;
use crate::repro::Route;
fn parse(args: &[&str]) -> Result<LaunchOptions, String> {
    LaunchOptions::parse(args.iter().map(OsString::from))
}
#[test]
fn defaults_and_paths_are_explicit() {
    let options = parse(&[]).unwrap();
    assert_eq!(options.fps, 60);
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
fn malformed_options_fail_before_startup() {
    for args in [
        vec!["--typo"],
        vec!["--fps"],
        vec!["--fps", "--performance-open"],
        vec!["--fps", "14"],
        vec!["--fps", "60", "--fps", "120"],
        vec!["--upscaler", "magic"],
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
    assert_eq!(options.repro.unwrap().route, Route::GrassSoak);
    assert_eq!(options.prepared_blades, Some(262144));
    assert!(options.audit_log);
    for route in Route::ALL {
        let name = route.name();
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
        if route.needs_start_view() {
            assert!(parse(&args).is_err(), "{name} needs a start view");
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
#[test]
fn time_passes_in_play_and_holds_in_measurements() {
    let play = parse(&[]).unwrap();
    assert!(play.day_clock);
    assert_eq!(play.time, None);
    assert!(!parse(&["--render-repro", "grass-close"]).unwrap().day_clock);
    assert!(!parse(&["--profile-seconds", "10"]).unwrap().day_clock);
    assert!(
        parse(&["--profile-seconds", "10", "--day-clock", "on"])
            .unwrap()
            .day_clock
    );
    assert_eq!(parse(&["--time", "18:00"]).unwrap().time, Some(0.75));
    assert_eq!(
        parse(&["--time", "06:30"]).unwrap().time,
        Some(390.0 / 1440.0)
    );
    for bad in ["24:00", "7", "7:60", "-1:00", "07:300"] {
        assert!(parse(&["--time", bad]).is_err(), "{bad}");
    }
    assert!(parse(&["--day-clock", "fast"]).is_err());
}

#[test]
fn one_mode_holds_the_camera_and_ends_the_run() {
    let mode = |args: &[&str]| parse(args).unwrap().mode;
    assert_eq!(mode(&[]), RunMode::Play);
    assert_eq!(mode(&["--render-repro", "grass-close"]), RunMode::Repro);
    assert_eq!(mode(&["--profile-seconds", "10"]), RunMode::Profile);
    assert_eq!(
        mode(&["--profile-seconds", "10", "--render-repro", "grass-soak"]),
        RunMode::Repro
    );
    let lab = ["--lod-lab", "pack/tree", "--start-view", "view.ron"];
    assert_eq!(mode(&lab), RunMode::LodLab);
    let look = [
        "--look-capture",
        "out",
        "--look-variants",
        "a",
        "--start-view",
        "view.ron",
    ];
    assert_eq!(mode(&look), RunMode::LookCapture);
    assert_eq!(mode(&["--streaming-smoke"]), RunMode::Smoke);
    let capture =
        std::env::temp_dir().join(format!("yarra-launch-{}.gputrace", std::process::id()));
    let capture = capture.to_str().unwrap();
    if cfg!(target_vendor = "apple") {
        let options = parse(&["--metal-capture", capture]).unwrap();
        assert_eq!(options.mode, RunMode::Play);
        // A capture is a measurement: authored weather, held time.
        assert_eq!(options.weather, engine::WeatherStart::Authored);
        assert!(!options.day_clock);
    }
    fn with<'a>(base: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
        [base, extra].concat()
    }
    for args in [
        with(&look, &["--streaming-smoke"]),
        with(&look, &["--metal-capture", capture]),
        with(&look, &["--profile-seconds", "10"]),
        with(&look, &["--render-repro", "grass-close"]),
        with(&lab, &["--metal-capture", capture]),
        with(&lab, &["--profile-diagnostic", "--metal-capture", capture]),
        with(&lab, &["--render-repro", "grass-close"]),
        with(&lab, &["--streaming-smoke"]),
        vec!["--streaming-smoke", "--render-repro", "grass-close"],
        vec!["--streaming-smoke", "--metal-capture", capture],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}

#[test]
fn every_option_needs_what_it_configures() {
    for args in [
        vec!["--render-frame-clock"],
        vec!["--render-temporal-view", "motion"],
        vec!["--render-snapshot-frames", "600"],
        vec!["--lod-lab-yaws", "0"],
        vec!["--lod-lab-stand-count", "3"],
        vec!["--look-variants", "a"],
        vec!["--look-settle", "10"],
        vec!["--profile-objects", "off"],
        vec!["--profile-auto-exposure", "off"],
        vec!["--diagnostics", "panel", "--timing-log"],
        vec!["--diagnostics", "off", "--trace-camera-input"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    let repro = |extra: &[&str]| parse(&[&["--render-repro", "grass-close"], extra].concat());
    let options = repro(&["--render-frame-clock", "--render-temporal-view", "depth"])
        .unwrap()
        .repro
        .unwrap();
    assert!(options.frame_clock);
    assert_eq!(
        options.temporal_view,
        upscaling::temporal::TemporalDebug::Depth
    );
    assert!(repro(&["--render-temporal-view", "colour"]).is_err());
}

#[test]
fn lod_lab_and_look_capture_options_are_parsed_once() {
    let lab = |extra: &[&str]| {
        parse(
            &[
                &["--lod-lab", "pack/tree", "--start-view", "view.ron"],
                extra,
            ]
            .concat(),
        )
    };
    let options = lab(&[]).unwrap().lod_lab.unwrap();
    assert_eq!(options.asset, "pack/tree");
    assert_eq!(options.yaws, [0., 90., 180.]);
    assert_eq!(options.distances, [10., 25., 50., 100., 200., 400., 800.]);
    assert_eq!((options.pitch, options.scale, options.settle), (3., 1., 30));
    assert!(options.stand.is_none());
    let options = lab(&[
        "--lod-lab-stand",
        "pack/other",
        "--lod-lab-stand-count",
        "12",
        "--lod-lab-spacing",
        "4.5",
        "--lod-lab-yaws",
        "45, 135",
        "--lod-lab-scale",
        "0.5",
    ])
    .unwrap()
    .lod_lab
    .unwrap();
    assert_eq!(options.stand, Some(("pack/other".into(), 12, 4.5)));
    assert_eq!(options.yaws, [45., 135.]);
    assert_eq!(options.scale, 0.5);
    for extra in [
        &["--lod-lab-capture", "a", "--lod-lab-screenshot", "b.png"][..],
        &["--lod-lab-distances", "1"],
        &["--lod-lab-scale", "8"],
        &["--lod-lab-yaws", "north"],
    ] {
        assert!(lab(extra).is_err(), "{extra:?}");
    }
    assert!(parse(&["--lod-lab", "pack/tree"]).is_err());

    let look = parse(&[
        "--look-capture",
        "out",
        "--look-variants",
        "a;b:ev=14",
        "--look-settle",
        "40",
        "--start-view",
        "view.ron",
    ])
    .unwrap()
    .look_capture
    .unwrap();
    assert_eq!(look.variants.len(), 2);
    assert_eq!(look.settle, 40.0);
    assert!(parse(&["--look-capture", "out", "--look-variants", "a"]).is_err());
    assert!(parse(&["--look-capture", "out", "--start-view", "view.ron"]).is_err());
}

#[test]
fn presentation_options_and_profile_scale() {
    let options = parse(&[
        "--story",
        "story",
        "--tree-shadow-lod",
        "2",
        "--resolution-scale",
        "0.75",
    ])
    .unwrap();
    assert_eq!(options.story, Some(PathBuf::from("story")));
    assert_eq!(options.tree_shadow_lod, 2);
    assert_eq!(options.resolution_scale, Some(0.75));
    for args in [
        vec!["--tree-shadow-lod", "3"],
        vec!["--resolution-scale", "0.6"],
        vec!["--day-clock", "fast"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    let profile = |extra: &[&str]| parse(&[&["--profile-seconds", "10"], extra].concat());
    let scaled = profile(&["--profile-size", "game", "--resolution-scale", "0.75"])
        .unwrap()
        .profile
        .unwrap();
    assert_eq!(scaled.resolution_scale(), 0.75);
    let default = profile(&["--profile-size", "game"])
        .unwrap()
        .profile
        .unwrap();
    assert_eq!(default.resolution_scale(), 0.5);
    // A fixed pixel size has no scale to honour.
    assert!(profile(&["--resolution-scale", "0.75"]).is_err());
    let switches = profile(&[
        "--profile-objects",
        "off",
        "--profile-terrain",
        "off",
        "--profile-fog",
        "off",
        "--profile-particles",
        "off",
        "--profile-shafts",
        "off",
        "--profile-auto-exposure",
        "off",
    ])
    .unwrap()
    .profile
    .unwrap();
    assert!(!switches.objects && !switches.terrain && !switches.fog);
    assert!(!switches.particles && !switches.light_shafts && !switches.auto_exposure);
    assert!(switches.bloom && switches.grass);
    assert!(profile(&["--profile-fog", "dim"]).is_err());
}

#[test]
fn help_and_the_performance_guide_list_every_option() {
    assert!(LaunchOptions::help().contains("\nScripted routes:\n  --render-repro NAME\n"));
    let table = flags::markdown_table();
    let guide = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/PERFORMANCE.md"
    ))
    .unwrap();
    assert!(
        guide.contains(&table),
        "docs/PERFORMANCE.md's launch table is out of date; replace it with:\n\n{table}"
    );
}
