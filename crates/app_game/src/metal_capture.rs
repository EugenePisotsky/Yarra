//! Opt-in, one-frame Metal capture for offline inspection in Xcode.
//! Launch with MTL_CAPTURE_ENABLED=1 and --metal-capture /absolute/path.gputrace.
//! On iOS a plain filename is saved under Documents/MetalCaptures in the app container.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bevy::{
    prelude::*,
    render::{
        RenderApp,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        renderer::{RenderDevice, RenderGraph, RenderGraphSystems},
        view::ExtractedView,
    },
};
use objc2_foundation::{NSString, NSURL};
use objc2_metal::{MTLCaptureDescriptor, MTLCaptureDestination, MTLCaptureManager};

#[derive(Resource, Clone, Default, ExtractResource)]
struct CaptureMainFrame(u32);

fn record_main_frame(
    frame: Res<bevy::diagnostic::FrameCount>,
    mut recorded: ResMut<CaptureMainFrame>,
) {
    recorded.0 = frame.0;
}

#[derive(Resource, Clone)]
struct CaptureDone(Arc<AtomicBool>);

#[derive(Resource)]
struct CaptureState {
    path: std::path::PathBuf,
    frame: u32,
    capturing: bool,
}

pub(super) fn install(app: &mut App) {
    let mut arguments = std::env::args_os();
    let Some(path) = arguments.find(|arg| arg == "--metal-capture").map(|_| {
        std::path::PathBuf::from(arguments.next().expect("--metal-capture requires a path"))
    }) else {
        return;
    };
    #[cfg(target_os = "ios")]
    let path = if path.is_absolute() {
        path
    } else {
        assert!(
            path.components().count() == 1 && path.file_name().is_some(),
            "iOS Metal capture requires an absolute path or a plain filename"
        );
        let directory = std::path::PathBuf::from(objc2_foundation::NSHomeDirectory().to_string())
            .join("Documents/MetalCaptures");
        std::fs::create_dir_all(&directory).expect("create Metal capture directory");
        directory.join(path)
    };
    assert!(path.is_absolute(), "Metal capture path must be absolute");
    assert!(!path.exists(), "Metal capture output already exists");
    assert_eq!(path.extension().and_then(|s| s.to_str()), Some("gputrace"));
    let done = CaptureDone(Arc::new(AtomicBool::new(false)));
    app.init_resource::<CaptureMainFrame>()
        .add_plugins(ExtractResourcePlugin::<CaptureMainFrame>::default())
        .add_systems(PostUpdate, record_main_frame);
    app.insert_resource(done.clone())
        .add_systems(Update, exit_after_capture);
    app.sub_app_mut(RenderApp)
        .insert_resource(done)
        .insert_resource(CaptureState {
            path,
            frame: 0,
            capturing: false,
        })
        .add_systems(
            RenderGraph,
            (
                begin_capture.before(RenderGraphSystems::Begin),
                end_capture.after(RenderGraphSystems::Finish),
            ),
        );
}

fn begin_capture(
    device: Res<RenderDevice>,
    mut state: ResMut<CaptureState>,
    done: Res<CaptureDone>,
    main_frame: Res<CaptureMainFrame>,
    views: Query<&ExtractedView, With<vegetation_render::VegetationDebugView>>,
    wind: Res<vegetation_render::VegetationWind>,
) {
    state.frame += 1;
    // Allow assets and pipelines to settle before recording; leave normal runs untouched.
    if main_frame.0 < 600 || state.capturing || done.0.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: only inspect the actual Metal device; no resources or command queues are mutated
    // through the HAL handle. The descriptor retains this valid MTLDevice during capture.
    let result = unsafe {
        let manager = MTLCaptureManager::sharedCaptureManager();
        let destination = MTLCaptureDestination::GPUTraceDocument;
        if !manager.supportsDestination(destination) {
            Err("Metal capture is unavailable; launch with MTL_CAPTURE_ENABLED=1".to_owned())
        } else if let Some(metal) = device.wgpu_device().as_hal::<wgpu::hal::api::Metal>() {
            let descriptor = MTLCaptureDescriptor::new();
            descriptor.setCaptureObject(Some(metal.raw_device().as_ref()));
            descriptor.setDestination(destination);
            descriptor.setOutputURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
                state
                    .path
                    .to_str()
                    .expect("Metal capture path must be UTF-8"),
            ))));
            manager
                .startCaptureWithDescriptor_error(&descriptor)
                .map_err(|e| e.to_string())
        } else {
            Err("Selected renderer is not Metal".to_owned())
        }
    };
    match result {
        Ok(()) => {
            state.capturing = true;
            for view in &views {
                warn!(
                    "METAL_CAPTURE view main_frame={} render_frame={} camera={:?} viewport={:?} wind_phase={}",
                    main_frame.0,
                    state.frame,
                    view.world_from_view.translation(),
                    view.viewport,
                    wind.phase_seconds()
                );
            }
            warn!(
                "METAL_CAPTURE started main_frame={} render_frame={} output={}",
                main_frame.0,
                state.frame,
                state.path.display()
            );
        }
        Err(error) => {
            error!("METAL_CAPTURE failed: {error}");
            done.0.store(true, Ordering::Release);
        }
    }
}

fn end_capture(mut state: ResMut<CaptureState>, done: Res<CaptureDone>) {
    if !state.capturing {
        return;
    }
    // SAFETY: this is the capture started by begin_capture in this render frame. All render
    // graph command buffers have been submitted before stopping the capture.
    unsafe {
        MTLCaptureManager::sharedCaptureManager().stopCapture();
    }
    state.capturing = false;
    #[cfg(target_os = "ios")]
    {
        // CoreDevice's app-container transfer refuses symlinks (ELOOP). Metal deduplicates
        // buffer snapshots with links inside the trace; materialize those bytes only after
        // capture ends so `devicectl device copy from` can transfer a self-contained trace.
        match materialize_capture_links(&state.path) {
            Ok(()) => eprintln!("METAL_CAPTURE saved output={}", state.path.display()),
            Err(error) => eprintln!("METAL_CAPTURE transfer preparation failed: {error}"),
        }
    }
    warn!("METAL_CAPTURE saved output={}", state.path.display());
    done.0.store(true, Ordering::Release);
}

fn exit_after_capture(done: Res<CaptureDone>, mut exit: MessageWriter<AppExit>) {
    if done.0.load(Ordering::Acquire) {
        exit.write(AppExit::Success);
        // winit's iOS event loop cannot terminate itself. This explicit capture-only launch
        // has finished writing the trace; end it so devicectl --console returns as on macOS.
        #[cfg(target_os = "ios")]
        std::process::exit(0);
    }
}

#[cfg(any(target_os = "ios", test))]
fn materialize_capture_links(directory: &std::path::Path) -> std::io::Result<()> {
    let root = directory.canonicalize()?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_symlink() {
            continue;
        }
        let source = entry.path().canonicalize()?;
        if !source.starts_with(&root) || !source.is_file() {
            return Err(std::io::Error::other("capture link leaves its directory"));
        }
        let temporary = entry.path().with_extension("transfer-tmp");
        std::fs::copy(source, &temporary)?;
        std::fs::rename(temporary, entry.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_transfer_preserves_linked_bytes() {
        let directory =
            std::env::temp_dir().join(format!("yarra-capture-links-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("original"), b"captured buffer").unwrap();
        std::os::unix::fs::symlink("original", directory.join("duplicate")).unwrap();
        materialize_capture_links(&directory).unwrap();
        assert!(
            !directory
                .join("duplicate")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(directory.join("duplicate")).unwrap(),
            b"captured buffer"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
