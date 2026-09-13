# Blade-band experiment — 2026-09-13

Status: checkpointed playtest, still subject to visual review. Desktop game launches now start at Medium at the user's request; B cycles Off / Subtle / Medium. iOS and old study documents still default to Off. This does not identify Yotei's implementation. The purpose is to test moving, irregular dark marks on grass without tracing neighboring blades or changing geometry.

## Review

- Local report: `.editor/vegetation/experiments/blade-bands-03/final/comparison.html`.
- Desktop game: `cargo run --release -p yarra-app-game`; use B for A/B or `--grass-bands off|subtle|medium` for an explicit starting mode. The bottom-right label shows the active mode. This uses the same shader as the study.
- Color editing: the study toolbar's **Colors…** opens root/tip pickers and clump variation for every active species in the selected population. These edit the same validated catalog draft as the inspector. Save study stores a local snapshot; Inspector → Save & Publish publishes the chosen colors to the runtime catalog. No color changes are applied merely by opening the window. `tools/vegetation_study.py ... --colors` opens it directly.
- Editor: **Vegetation → Blade bands → Off / Subtle / Medium / Band mask / Motion mask (fixed blades)**. Play uses the existing wind transport. Pause/step/time apply to geometry and marks together.
- Reproduce the 28 A/B captures plus 12 motion diagnostic frames: `python3 tools/grass_blade_band_study.py --previous .editor/vegetation/experiments/blade-bands-02/final --motion-frames 12 --output .editor/vegetation/experiments/blade-bands-next`.
- Open the captured dense view with motion:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/blade-bands-03/final/dense-0-medium/study.ron \
  --blade-bands medium --play --no-character
```

The comparison keeps the previous detailed-understory ground treatment fixed. Ground patch contrast has not been retuned in this experiment. All captures use production geometry LOD, 1280 × 720, MSAA off, and a 16 m study field. Sparse source density is 11 roots/m²; dense is 44. Those source densities differ between scenes, never between band variants of a scene.

## Pattern and motion

`vegetation_debug_draw.wgsl` specializes an enabled-only vertex/fragment path:

1. Hash the stable 24-bit blade seed plus pair-member index. Exclude compacted instance index and the high byte containing LOD retention. The marks do not get randomized each frame.
2. Carry physical curve parameter `t`, estimated height relative to the blade crown, authored width relative to length, and bounded wind drift to the fragment shader. The crown estimate samples the current cubic at t=0.5, 0.75 and 1.0; it is deliberately cheap and approximate.
3. Use a warped blade coordinate with 5–10 potential cells, random phase and random root-to-tip compression per blade. The sampling coordinate is `s = t - 1.25*wind_drift`; the cell coordinate is `count*s*(1+warp-warp*s)+phase`. Advecting before cell selection lets marks cross the old fixed slots without jumping or being regenerated each frame.
4. Evaluate the containing cell and its two neighbors. Each mark has its own stable angle, center (0.15–0.85 cell), width, intensity and presence. Even at full source density, some marks are absent. Neighbor evaluations allow slanted marks to cross cell boundaries continuously. Bounds limit a mark's reach to one cell, so three candidates cover all contributors. Overlaps combine with `max`, avoiding accumulated blanket darkness.
5. Keep low marks stronger and more likely than high marks. The immediate root point fades out (t=0.005–0.025). Fade near the estimated crown; source density controls presence independently of camera LOD. This is the selected population's global density, **not** local canopy occupancy.
6. Drive bounded motion with the existing two sine components and wind time, speed, direction, frequency and strength. Maximum drift is 1.25 authored width-to-length units in curve parameter, scaled by wind strength (an approximate physical width because curve speed varies). Wind off stops it. Different blade seeds give different phases, while a blade's mark identities remain stable.
7. Compute screen derivatives of continuous longitudinal/across-blade coordinates before floor/hash. Combine those derivatives with each mark's angle to filter its edges; fade marks when unresolved (0.7–2 pixels). There is no new distance ring or topology switch.

**Motion mask** holds the blade shape at wind phase zero while the draw shader keeps the live wind clock. Both prepared geometry and the overflow fallback use the same fixed time. The preparation cache reuses the frozen shape, and switching back to Medium restores normal wind. This diagnostic isolates sliding marks from grass movement; use Medium for the actual material result. The report's separate motion scrubber compares the fixed shape at twelve mark phases.

Subtle can remove up to 54% of direct light at a fully covered low mark; Medium up to 82%. Ambient/body attenuation is 65% of that amount (up to about 35% / 53%). Both are additionally weakened by height, density, band shape and pixel-size masks. Existing root AO, shadow reception and ground treatment are retained. Band mask shows the effective spatial mask before the Subtle/Medium strength, so it intentionally has more contrast than the material result.

The first direct-light-only probe was too faint under the existing ambient fill. Restrained body attenuation made the marks readable without uniformly darkening the field. The first version remained too hard to see: three candidate marks plus strong crown/root suppression usually left one visible line. The second version increased contrast, but its regular cells and single angle per blade produced stripes; its drift was capped to 0.08 cell with further random attenuation and looked static. This third version addresses those two failures. It remains an approximation, not an accepted physical shadow solution.

## Cost and scope

- No additional grass vertices, indices, instances, textures, samples, geometry generation, or shadow passes.
- Enabled variant adds a `vec4<f32>` and flat `u32` shader varying (five scalar components, nominal 20 bytes per vertex output), vertex arithmetic including two sine evaluations, and three neighboring cell hashes/evaluations per shaded fragment. This increases fragment arithmetic from the second version's single cell; the evaluation count remains fixed regardless of the number of potential marks. There is no measured GPU-time delta yet. Interpolation/register pressure and fragment ALU are real costs; this is not free.
- Off specializes out the band functions/varyings and folds visibility to one. The GPU config buffer remains 32 bytes. A spare byte stores source density; placement-cache comparison ignores this shading-only byte. Bit 9 enables the fixed-geometry diagnostic and participates in preparation invalidation.
- Settings persist in local study RON and restore with the workspace's existing setting ownership. The playtest changes the desktop game's startup band mode only. No population catalog or runtime database is modified automatically.

`--profile` now writes available native render histories to `render-timings.txt` after 360 ready frames. The M2 Max Metal device exposes `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS`, but **not** `TIMESTAMP_QUERY_INSIDE_PASSES`. The grass draw span therefore has no `elapsed_gpu` samples. Other reported zero-duration GPU entries are not useful grass measurements. Do not use CPU submission times, editor FPS, or these zeros as the experiment's GPU cost. A native Metal/Xcode capture or a backend with pass timestamps is still needed to quantify the cost and decide whether to retain the desktop playtest default.

Attempt and device evidence: `.editor/vegetation/experiments/blade-bands-01/profile-medium-2/render-timings.txt`.

## Verification and limitations

- Desktop playtest integration: release game and debug editor built; 10 game tests and 23 editor vegetation tests passed. Native game Medium rendering was inspected with production MSAA and streamed pages. Medium/Off logs both reported 66,289 grass units and 764,175 indices. The first unfocused Off window capture was black (including UI), so it was excluded from pixel comparisons; those runs are not a GPU-cost measurement.
- The Colors window was inspected in a native editor capture and opened for live editing. It reuses the inspector's color widgets and validated draft path.
- Renderer tests: 22 passed, 8 native/opt-in tests ignored. WGSL validation checks all five variants; the motion-cache test checks fixed-time reuse and restoring normal motion; previous shadow fixture still validates with bands disabled.
- Native prepared-versus-fallback regression: passed with wind, MSAA and forced overflow (`blade-bands-03/preparation-test.log`).
- Editor vegetation-related tests: 23 passed, including old checkpoint defaults, replay, exact wind transport and workspace restoration.
- 28 actual editor captures: identical emitted topology-bin counts and ground-coverage hashes within each comparison scene.
- Ground pixels distinguishable from the monochrome grass mask stayed byte-identical in every A/B: 379,531–772,812 verified pixels per first-version scene. The second-version checks are stored beside its report in `pixel-checks.json`.
- First version: Medium changed more than one code value on 45,617 pixels in the dense close wind-0 scene, versus 4,294 in the quarter-density scene.
- Second-version probe, same dense view: 52,707 pixels darkened by more than 10 code values in at least one channel, versus 15,149 previously (3.48×). Maximum channel darkening rose from 30 to 51. This measures visible coverage/contrast, not the number of distinct lines or GPU cost. Isolated crossing/rasterization differences also occur in repeated Off captures, so image differences are not a pure shadow mask.
- Third version: 28 A/B captures plus 12 fixed-geometry motion masks. All retained identical emitted bins and ground-coverage hashes. Normal A/B comparisons preserved 379,531–772,812 identifiable ground pixels per scene.
- Fixed-blade sequence: grass support was pixel-identical throughout 0–2.75 s. At 0.25 s, 40,727 mask pixels differed by more than 10 channel values from phase zero; at 0.5 s, 64,753 differed. All identifiable ground pixels were unchanged. This isolates movement over stationary blades instead of mistaking geometry sway for mark motion.
- Wind-off phase 0 versus 1 s: only 16 pixels differed by more than one code value, maximum 7, consistent with the previously observed crossing/rasterization variation. There was no broad mark movement.
- Third-version dense close Medium darkened 30,438 pixels by more than 10 code values, versus 52,706 in version two. Reduced regular coverage is deliberate: more irregular spacing and omitted marks, not an increase in blanket darkness. Lower marks retain the same light-attenuation limits.
- The report's wind button alternates frozen poses, not continuous playback. Use the editor to judge motion and camera stability.

There is no actual above-blade visibility query. Short blades do not know about a taller neighboring canopy, and isolated edge blades can receive false marks. Blade-local marks can also read as material stripes when examined closely. If that remains visible during motion, improve the placement/height model before making them darker or adding more bands. A more localized ground material is a separate follow-up so it cannot disguise whether these blade marks help.
