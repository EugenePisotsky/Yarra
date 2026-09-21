//! Bounded, opt-in AppKit → camera trace. No event is modified or swallowed.
use bevy::{
    prelude::*,
    window::{PrimaryWindow, RawHandleWrapper},
};
use block2::RcBlock;
use engine::CameraInputDiagnostics;
use objc2::{Message as _, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSView};
use objc2_foundation::NSProcessInfo;
use raw_window_handle::RawWindowHandle;
use std::{
    cell::RefCell,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    ptr::NonNull,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RECORD_SECONDS: u64 = 20;
const MAX_EVENTS: usize = 16_384;
const MAX_FRAMES: usize = 8_192;

pub(super) fn install(app: &mut App) {
    if !std::env::args_os().any(|a| a == "--trace-camera-input") {
        return;
    }
    app.init_resource::<CameraInputDiagnostics>()
        .add_systems(Startup, start)
        .add_systems(PostUpdate, record);
}

struct NativeEvent {
    received: Instant,
    timestamp: f64,
    delivered_uptime: f64,
    pixel: bool,
    x: f32,
    y: f32,
    phase: usize,
    momentum_phase: usize,
}

#[derive(Default)]
struct NativeEvents {
    events: Vec<NativeEvent>,
    started: Option<Instant>,
    overflow: bool,
}

struct Frame {
    at: Instant,
    uptime: f64,
    native_end: usize,
    camera: CameraInputDiagnostics,
}

// AppKit monitor ownership and the event buffer stay on the main thread.
struct Trace {
    origin: Instant,
    monitor: Option<Retained<AnyObject>>,
    native: Rc<RefCell<NativeEvents>>,
    frames: Vec<Frame>,
    directory: PathBuf,
    announced: bool,
}

fn start(world: &mut World) {
    let origin = Instant::now();
    let mut windows = world.query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>();
    let Ok(handle) = windows.single(world) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
        return;
    };
    // SAFETY: Startup runs on the main thread, and the primary window owns this view.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref().retain() };
    let Some(window) = view.window() else { return };
    let number = window.windowNumber();
    let native = Rc::new(RefCell::new(NativeEvents::default()));
    let events = native.clone();
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit supplies a valid event for the duration of the callback.
        let e = unsafe { event.as_ref() };
        if e.windowNumber() == number {
            let received = Instant::now();
            let pixel = e.hasPreciseScrollingDeltas();
            // Match winit's logical→physical conversion, including Retina scale.
            let scale = if pixel {
                view.window().map_or(1.0, |w| w.backingScaleFactor())
            } else {
                1.0
            };
            let x = (e.scrollingDeltaX() * scale) as f32;
            let y = (e.scrollingDeltaY() * scale) as f32;
            let mut events = events.borrow_mut();
            if events.started.is_none() && (x != 0.0 || y != 0.0) {
                events.started = Some(received);
            }
            if events.events.len() < MAX_EVENTS {
                events.events.push(NativeEvent {
                    received,
                    timestamp: e.timestamp(),
                    delivered_uptime: NSProcessInfo::processInfo().systemUptime(),
                    pixel,
                    x,
                    y,
                    phase: e.phase().0,
                    momentum_phase: e.momentumPhase().0,
                });
            } else {
                events.overflow = true;
            }
        }
        // Always pass the exact original event through to winit.
        event.as_ptr()
    });
    // SAFETY: the block returns the original live event; AppKit retains the block.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &handler)
    };
    let Some(monitor) = monitor else {
        warn!("CAMERA_INPUT_TRACE could not install AppKit monitor");
        return;
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let directory = std::env::current_dir()
        .expect("working directory")
        .join(format!("tmp/camera-input-{stamp}-{}", std::process::id()));
    warn!(
        "CAMERA_INPUT_TRACE armed: records {RECORD_SECONDS}s from the first scroll, then saves to {}",
        directory.display()
    );
    world.insert_non_send(Trace {
        origin,
        monitor: Some(monitor),
        native,
        frames: Vec::new(),
        directory,
        announced: false,
    });
}

fn record(trace: Option<NonSendMut<Trace>>, camera: Res<CameraInputDiagnostics>) {
    let Some(mut trace) = trace else { return };
    if trace.monitor.is_none() {
        return;
    }
    let now = Instant::now();
    let (started, native_end, overflow) = {
        let mut native = trace.native.borrow_mut();
        // Still capture if AppKit's monitor misses an event seen by the camera;
        // otherwise the very delivery mismatch we are investigating could keep
        // the trace from ever starting. Ignore phase-only zero wheel events.
        if native.started.is_none()
            && (camera.pixel_delta != Vec2::ZERO || camera.line_delta != Vec2::ZERO)
        {
            native.started = Some(now);
        }
        (native.started, native.events.len(), native.overflow)
    };
    // Don't grow a frame buffer while waiting for the user to start a gesture.
    if started.is_some() || native_end > trace.frames.last().map_or(0, |f| f.native_end) {
        if started.is_some() && !trace.announced {
            warn!("CAMERA_INPUT_TRACE recording; rotate normally for {RECORD_SECONDS}s");
            trace.announced = true;
        }
        trace.frames.push(Frame {
            at: now,
            uptime: NSProcessInfo::processInfo().systemUptime(),
            native_end,
            camera: camera.clone(),
        });
    }
    if started.is_some_and(|s| now.duration_since(s) >= Duration::from_secs(RECORD_SECONDS))
        || overflow
        || trace.frames.len() >= MAX_FRAMES
    {
        trace.finish();
    }
}

impl Trace {
    fn finish(&mut self) {
        let Some(monitor) = self.monitor.take() else {
            return;
        };
        // SAFETY: this is exactly the monitor token returned by AppKit, removed once on main.
        unsafe { NSEvent::removeMonitor(&monitor) };
        match self.save() {
            Ok(()) => warn!(
                "CAMERA_INPUT_TRACE saved {} (native_events={}, frames={}, overflow={})",
                self.directory.display(),
                self.native.borrow().events.len(),
                self.frames.len(),
                self.native.borrow().overflow
            ),
            Err(error) => warn!("CAMERA_INPUT_TRACE save failed: {error}"),
        }
    }

    fn save(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let native = self.native.borrow();
        let origin = self.origin;
        let mut events = BufWriter::new(File::create(self.directory.join("native.csv"))?);
        writeln!(
            events,
            "event,received_ms,event_uptime_s,dispatch_delay_ms,pixel,x,y,phase,momentum_phase"
        )?;
        for (i, e) in native.events.iter().enumerate() {
            writeln!(
                events,
                "{i},{:.6},{:.9},{:.6},{},{:.6},{:.6},{},{}",
                e.received.duration_since(origin).as_secs_f64() * 1000.0,
                e.timestamp,
                (e.delivered_uptime - e.timestamp) * 1000.0,
                e.pixel,
                e.x,
                e.y,
                e.phase,
                e.momentum_phase
            )?;
        }
        events.flush()?;
        let mut frames = BufWriter::new(File::create(self.directory.join("frames.csv"))?);
        writeln!(
            frames,
            "frame,at_ms,wall_dt_ms,camera_dt_ms,input_read_dt_ms,native_end,native_count,camera_count,pixel_x,pixel_y,line_x,line_y,clipped,enabled,pointer_blocked,startup_guard,requested_orbit,applied_orbit,yaw_before,yaw_after,target_yaw,oldest_event_age_ms,newest_event_age_ms,oldest_dispatch_to_read_ms,newest_dispatch_to_read_ms"
        )?;
        let mut previous: Option<&Frame> = None;
        for f in &self.frames {
            let d = &f.camera;
            let begin = previous.map_or(0, |p| p.native_end);
            let batch = &native.events[begin..f.native_end];
            let age =
                |e: Option<&NativeEvent>| e.map_or(f64::NAN, |e| (f.uptime - e.timestamp) * 1000.0);
            let dispatch_to_read = |e: Option<&NativeEvent>| {
                e.zip(d.read_at).map_or(f64::NAN, |(e, read)| {
                    if read >= e.received {
                        read.duration_since(e.received).as_secs_f64() * 1000.0
                    } else {
                        -e.received.duration_since(read).as_secs_f64() * 1000.0
                    }
                })
            };
            let wall_dt = previous.map_or(f64::NAN, |p| {
                f.at.duration_since(p.at).as_secs_f64() * 1000.0
            });
            let read_dt = previous
                .and_then(|p| p.camera.read_at.zip(d.read_at))
                .map_or(f64::NAN, |(p, c)| {
                    c.duration_since(p).as_secs_f64() * 1000.0
                });
            writeln!(
                frames,
                "{},{:.6},{wall_dt:.6},{:.6},{read_dt:.6},{},{},{},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{:.9},{:.9},{:.9},{:.9},{:.9},{:.6},{:.6},{:.6},{:.6}",
                d.sequence,
                f.at.duration_since(origin).as_secs_f64() * 1000.0,
                d.dt_secs * 1000.0,
                f.native_end,
                batch.len(),
                d.wheel_events,
                d.pixel_delta.x,
                d.pixel_delta.y,
                d.line_delta.x,
                d.line_delta.y,
                d.clipped_events,
                d.enabled,
                d.pointer_blocked,
                d.startup_guard,
                d.requested_orbit,
                d.applied_orbit,
                d.yaw_before,
                d.yaw_after,
                d.target_yaw,
                age(batch.first()),
                age(batch.last()),
                dispatch_to_read(batch.first()),
                dispatch_to_read(batch.last())
            )?;
            previous = Some(f);
        }
        frames.flush()
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        self.finish();
    }
}
