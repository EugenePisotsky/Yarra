# Yarra Metal presentation interval

This is the crates.io source distribution of **wgpu-hal 29.0.4**, vendored under
its original MIT/Apache-2.0 licenses. The root Cargo manifest patches that exact
dependency so the change is reproducible without editing Cargo's shared cache.

The only source changes are in `src/metal/mod.rs` and `src/metal/adapter.rs`:

- Add a per-queue minimum presentation duration, disabled by default.
- Expose `Queue::set_minimum_presentation_duration(Duration)` to the application
  through wgpu's existing `Queue::as_hal` interface.
- When enabled and supported, use Metal's `presentDrawable:afterMinimumDuration:`
  (or the corresponding drawable method for transaction-based presentation).

The Cargo manifests also enable `objc2-metal/objc2-core-foundation`, which exposes
those duration-based methods. Declare it here so builds without MetalFX do not
depend on another crate enabling it through Cargo feature unification.

All other backends and queues that do not opt in retain upstream behavior. The
setter does not wait for the GPU or sleep. The application controls update pacing
separately. The interval is a minimum display duration, not a guarantee that an
over-budget frame can meet its deadline.

Remove this patch when upstream wgpu exposes the needed presentation policy.
When updating wgpu, rebase these changes and repeat the ProMotion presentation
interval test; an average FPS counter is insufficient to validate this feature.

Reference: [Apple's Metal presentation method](https://developer.apple.com/documentation/metal/mtlcommandbuffer/present(_:afterminimumduration:)).
