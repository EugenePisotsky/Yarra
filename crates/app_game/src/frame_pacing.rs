//! Runtime frame cap. One setting drives the event loop and Metal presentation.
use bevy::{
    prelude::*,
    winit::{UpdateMode, WinitSettings},
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameRate(u32);

impl FrameRate {
    pub(crate) fn new(fps: u32) -> Self {
        assert!(
            fps == 0 || (15..=240).contains(&fps),
            "FPS requires 0 or 15..240"
        );
        Self(fps)
    }

    pub(crate) fn fps(self) -> u32 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(match self.0 {
            0 => 30,
            30 => 60,
            60 => 120,
            120 => 0,
            _ => 0,
        })
    }

    pub(crate) fn label(self) -> String {
        if self.0 == 0 {
            "FPS limit: Follow display".into()
        } else {
            format!("FPS limit: {}", self.0)
        }
    }

    fn interval(self) -> Duration {
        if self.0 == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(1.0 / f64::from(self.0))
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FramePacing {
    pub(crate) rate: FrameRate,
    pub(crate) profile_locked: bool,
    timer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClockSource {
    Display,
    Timer,
    FollowDisplay,
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
struct AppliedPacing {
    rate: FrameRate,
    source: ClockSource,
    presentation_interval: Duration,
}

impl FramePacing {
    pub(crate) fn control_label(self) -> String {
        if self.profile_locked {
            format!("{} (profile locked)", self.rate.label())
        } else {
            self.rate.label()
        }
    }

    fn resolve(self, display_available: bool) -> AppliedPacing {
        let source = if self.rate.fps() == 0 {
            ClockSource::FollowDisplay
        } else if display_available && !self.timer {
            ClockSource::Display
        } else {
            ClockSource::Timer
        };
        AppliedPacing {
            rate: self.rate,
            source,
            presentation_interval: if source == ClockSource::Display {
                self.rate.interval()
            } else {
                Duration::ZERO
            },
        }
    }
}

impl AppliedPacing {
    fn event_loop(self, original: &WinitSettings) -> WinitSettings {
        if self.source == ClockSource::FollowDisplay {
            return original.clone();
        }
        let mode = UpdateMode::Reactive {
            wait: if self.source == ClockSource::Display {
                Duration::MAX
            } else {
                self.rate.interval()
            },
            react_to_device_events: false,
            react_to_user_events: self.source == ClockSource::Display,
            react_to_window_events: false,
        };
        WinitSettings {
            focused_mode: mode,
            unfocused_mode: mode,
        }
    }
}

#[derive(Resource)]
struct Controller {
    last: Option<FramePacing>,
    original_event_loop: WinitSettings,
}

pub(crate) fn install(app: &mut App, rate: FrameRate, timer: bool) {
    let original_event_loop = app.world().resource::<WinitSettings>().clone();
    app.insert_resource(FramePacing {
        rate,
        profile_locked: false,
        // Retain the old limiter for a controlled comparison.
        timer,
    })
    .insert_resource(Controller {
        last: None,
        original_event_loop,
    })
    .add_systems(PostStartup, apply_pending)
    .add_systems(
        PostUpdate,
        apply_pending.run_if(resource_changed::<FramePacing>),
    );
    #[cfg(target_os = "macos")]
    {
        app.add_systems(First, macos::acknowledge_frame);
        macos::install_presentation(app);
    }
}

pub(crate) fn set_fps(app: &mut App, fps: u32) {
    app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(fps);
}

// Exclusive system: all AppKit lifecycle work stays on the app/main thread.
// PostUpdate sees UI changes before extraction; render receives the applied policy.
fn apply_pending(world: &mut World) {
    let settings = *world.resource::<FramePacing>();
    if world.resource::<Controller>().last == Some(settings) {
        return;
    }
    #[cfg(target_os = "macos")]
    let display_available = macos::configure_clock(world, settings);
    #[cfg(not(target_os = "macos"))]
    let display_available = false;
    let applied = settings.resolve(display_available);
    let winit = applied.event_loop(&world.resource::<Controller>().original_event_loop);
    world.insert_resource(winit);
    world.insert_resource(applied);
    world.resource_mut::<Controller>().last = Some(settings);
    warn!(
        "FRAME_PACING target_fps={} clock={:?}",
        applied.rate.fps(),
        applied.source
    );
}

#[cfg(test)]
#[path = "frame_pacing/tests.rs"]
mod tests;

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use bevy::{
        render::{
            Extract, ExtractSchedule, Render, RenderApp, RenderSystems, renderer::RenderQueue,
        },
        window::{PrimaryWindow, RawHandleWrapper},
        winit::{EventLoopProxyWrapper, WinitUserEvent},
    };

    pub(super) fn install_presentation(app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .add_systems(ExtractSchedule, extract_settings)
                .add_systems(
                    Render,
                    configure_presentation.in_set(RenderSystems::PrepareViews),
                );
        }
    }

    fn extract_settings(mut commands: Commands, settings: Extract<Res<AppliedPacing>>) {
        if settings.is_changed() {
            commands.insert_resource(**settings);
        }
    }

    fn configure_presentation(settings: Option<Res<AppliedPacing>>, queue: Res<RenderQueue>) {
        let Some(settings) = settings else { return };
        if !settings.is_changed() && !queue.is_changed() {
            return;
        }
        let duration = settings.presentation_interval;
        // SAFETY: only configure this queue's presentation policy. No raw resource
        // is destroyed, submitted or retained beyond the HAL guard.
        if let Some(metal) = unsafe { queue.as_hal::<wgpu::hal::api::Metal>() } {
            metal.set_minimum_presentation_duration(duration);
            warn!(
                "FRAME_PACING metal_minimum_present_ms={:.6}",
                duration.as_secs_f64() * 1000.0
            );
        }
    }
    use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel};
    use objc2_app_kit::NSView;
    use objc2_foundation::{
        MainThreadMarker, NSObject, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo,
        NSRunLoop, NSRunLoopCommonModes,
    };
    use objc2_quartz_core::{CADisplayLink, CAFrameRateRange};
    use raw_window_handle::RawWindowHandle;
    use std::{
        ops::Deref,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread::JoinHandle,
    };

    struct Tick {
        wake: Box<dyn Fn() + Send + Sync>,
        pending: AtomicBool,
        seen: AtomicBool,
        active: AtomicBool,
    }

    impl Tick {
        fn request_frame(&self) {
            // A slow frame consumes the latest tick; never queue catch-up frames.
            if self.active.load(Ordering::Acquire) && !self.pending.swap(true, Ordering::Relaxed) {
                (self.wake)();
            }
        }

        fn set_active(&self, active: bool) {
            self.active.store(active, Ordering::Release);
            if !active {
                self.pending.store(false, Ordering::Relaxed);
            }
            self.seen.store(true, Ordering::Relaxed);
        }

        fn watchdog_tick(&self) {
            if !self.seen.swap(false, Ordering::Relaxed) {
                self.request_frame();
            }
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements. The callback runs on
        // the main run loop; its Objective-C signature is void (CADisplayLink *).
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = Arc<Tick>]
        struct YarraFramePacingTarget;

        unsafe impl NSObjectProtocol for YarraFramePacingTarget {}

        impl YarraFramePacingTarget {
            #[unsafe(method(displayTick:))]
            fn display_tick(&self, _link: &CADisplayLink) {
                self.ivars().seen.store(true, Ordering::Relaxed);
                self.ivars().request_frame();
            }
        }
    );

    // NonSend ownership keeps AppKit creation and invalidation on the main thread.
    pub(super) struct DisplayClock {
        link: Retained<CADisplayLink>,
        _target: Retained<YarraFramePacingTarget>,
        tick: Arc<Tick>,
        stop: mpsc::Sender<()>,
        watchdog: Option<JoinHandle<()>>,
    }

    impl Drop for DisplayClock {
        fn drop(&mut self) {
            self.tick.set_active(false);
            self.link.invalidate();
            let _ = self.stop.send(());
            if let Some(thread) = self.watchdog.take() {
                let _ = thread.join();
            }
        }
    }

    pub(super) fn configure_clock(world: &mut World, settings: FramePacing) -> bool {
        let requested = settings.rate.fps() != 0 && !settings.timer;
        if requested && world.get_non_send::<DisplayClock>().is_none() {
            if let Some(clock) = create(world) {
                world.insert_non_send(clock);
            }
        }
        let Some(clock) = world.get_non_send::<DisplayClock>() else {
            return false;
        };
        if requested {
            let fps = settings.rate.fps() as f32;
            clock.link.setPreferredFrameRateRange(CAFrameRateRange {
                minimum: fps,
                maximum: fps,
                preferred: fps,
            });
            clock.tick.set_active(true);
            clock.link.setPaused(false);
        } else {
            clock.tick.set_active(false);
            clock.link.setPaused(true);
        }
        requested
    }

    fn create(world: &mut World) -> Option<DisplayClock> {
        let mtm = MainThreadMarker::new().expect("display link must start on the main thread");
        if !NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 0,
            patchVersion: 0,
        }) {
            warn!("FRAME_PACING timer fallback: window display links require macOS 14");
            return None;
        }
        let mut query = world.query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>();
        let Ok(handle) = query.single(world) else {
            warn!("FRAME_PACING timer fallback: primary window has no native handle");
            return None;
        };
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            return None;
        };
        let proxy = world.resource::<EventLoopProxyWrapper>().deref().clone();
        let tick = Arc::new(Tick {
            wake: Box::new(move || {
                let _ = proxy.send_event(WinitUserEvent::WakeUp);
            }),
            pending: AtomicBool::new(false),
            seen: AtomicBool::new(true),
            active: AtomicBool::new(false),
        });
        let target = YarraFramePacingTarget::alloc(mtm).set_ivars(tick.clone());
        // SAFETY: init is inherited from NSObject. The NSView is owned by the
        // live primary window, accessed on the main thread. The selector matches
        // the target method above; the link retains its target until invalidated.
        let target: Retained<YarraFramePacingTarget> = unsafe { msg_send![super(target), init] };
        let link = unsafe {
            let view = handle.ns_view.cast::<NSView>().as_ref();
            view.displayLinkWithTarget_selector(&target, sel!(displayTick:))
        };
        link.setPaused(true);
        unsafe {
            link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }

        // A hidden/minimized view stops receiving display callbacks. Keep lifecycle
        // and close events responsive then, without adding a second active-frame
        // timer. This thread only requests an update after callbacks have stopped.
        let (stop, receiver) = mpsc::channel();
        let watchdog_tick = tick.clone();
        let watchdog = std::thread::Builder::new()
            .name("display-link-watchdog".into())
            .spawn(move || {
                while receiver.recv_timeout(Duration::from_millis(250))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    watchdog_tick.watchdog_tick();
                }
            })
            .expect("display-link watchdog");
        Some(DisplayClock {
            link,
            _target: target,
            tick,
            stop,
            watchdog: Some(watchdog),
        })
    }

    pub(super) fn acknowledge_frame(clock: Option<NonSend<DisplayClock>>) {
        if let Some(clock) = clock {
            clock.tick.pending.store(false, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;

        #[test]
        fn display_and_watchdog_ticks_do_not_queue_catch_up_frames() {
            let wakes = Arc::new(AtomicUsize::new(0));
            let counted = wakes.clone();
            let tick = Arc::new(Tick {
                wake: Box::new(move || {
                    counted.fetch_add(1, Ordering::Relaxed);
                }),
                pending: AtomicBool::new(false),
                seen: AtomicBool::new(true),
                active: AtomicBool::new(true),
            });
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    let tick = tick.clone();
                    scope.spawn(move || {
                        for _ in 0..100 {
                            tick.request_frame();
                        }
                    });
                }
            });
            assert_eq!(wakes.load(Ordering::Relaxed), 1);
            tick.pending.store(false, Ordering::Relaxed);
            tick.request_frame();
            assert_eq!(wakes.load(Ordering::Relaxed), 2);

            // Follow-display must not leave a second source of updates behind.
            tick.set_active(false);
            for _ in 0..5 {
                tick.watchdog_tick();
                tick.request_frame();
            }
            assert_eq!(wakes.load(Ordering::Relaxed), 2);
            tick.set_active(true);
            tick.request_frame();
            tick.request_frame();
            assert_eq!(wakes.load(Ordering::Relaxed), 3);
        }
    }
}
