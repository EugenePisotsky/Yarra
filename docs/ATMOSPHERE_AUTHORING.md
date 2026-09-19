# Atmosphere authoring and runtime pipeline

Status: first usable slice implemented, 2026-09-19.
[Architecture and remaining slices](ATMOSPHERE_ARCHITECTURE.md).

## Open and preview

In the **World** workspace, open **Tools → Atmosphere**. It uses the existing
landscape and camera. There is one atmosphere profile per world; opening this
window does not change the painting tool or spatial source demand.

**Preview · temporary** contains:

- Sunrise, Day, Sunset and Night shortcuts, an hour scrubber, Play/Pause and speed.
  Sunrise/Sunset show a low visible sun; the geometric horizon crossings are 6:00
  and 18:00 on the current normalized cycle.
- Edited / Published comparison at the same time and camera.
- Reset preview, which restores preview defaults without reverting source edits.
- An optional exposure lock that captures the current evaluated exposure, independent
  of the authored day/night settings.
- A visible, clearable bookmark visibility override when launched with one.

Preview starts paused. Time, playback speed, comparison and exposure lock never
mark the project dirty and are not saved in the world profile. Playback speed is
an inspection aid; authored day length supplies the cycle duration. Controls are
retained per world during the editor session. Leaving World pauses playback.

## Author the appearance

**World settings · saved** contains:

| Group | Controls |
| --- | --- |
| Outdoor policy | Enable/disable outdoor sky and sunlight for this world |
| Sun and sky | Sun-path heading, noon elevation, disk diameter, molecular air scattering |
| Day palette | Explicit Night/Sunrise/Day/Sunset edit target; sunlight and ambient tint/intensity |
| Night lighting | Moon enablement, heading/elevation, disk size, tint/intensity, night EV100 |
| Haze | Aerosol visibility and tint |
| Presentation | Day exposure EV100 and bloom strength |
| Startup and cycle | Initial phase, Use preview as startup, day length |

Selecting a palette target does not change preview time. **Preview this phase**
provides that explicit action. Sunlight and ambient color are independent, allowing
warm sunrise light with cooler shadows. Scattering and presentation affect the
result as well; these are not literal sky-gradient color keys.

Drag a property to preview it on the landscape. A completed gesture creates one
undo command in the shared chronological editor history. Numeric typing completes
on Enter or when focus leaves the field. Escape cancels the current gesture.
**Revert unsaved atmosphere edits** is also undoable. Undo/redo, terrain/object
edits and atmosphere edits share the existing history controls.

Completed unsaved atmosphere edits participate in recovery. Invalid profiles
cannot enter the renderer or source database. Controls constrain numeric values
and source loading/cooking validate ranges and bounded records.

To tune night, open **Night lighting → Preview night**. Adjust moon strength/tint
for the lit surfaces and gloss, then Night exposure for overall visibility. Select
Night in Day palette to adjust shadow fill. Light and exposure blend through
twilight; the daylight settings are unchanged. The current moon uses a simple disk
and fixed position, with no stars, lunar phases or surface texture yet.

A typical session:

1. Preview Sunrise, choose the Sunrise palette, and tune sunlight/ambient colors.
2. Scrub the morning transition without creating source changes.
3. Adjust Haze visibility and compare at Day and Sunset. Clear any bookmark
   override first if it is masking the saved visibility.
4. Switch Edited / Published and lock exposure when comparing looks.
5. Save, or use Save & Publish to make the result available to the game.

Saving while viewing midnight does not change the game's initial time. Use the
explicit startup field or **Use preview as startup** for that. Weather presets
and weather blending are not present yet; Haze edits the current world's profile.

## Save, publish and play

Save writes the current profile to the project database in the background using
its expected source revision. A conflicting write preserves local changes and
shows **Keep my edits / Use database**. Resolving a conflict clears the old shared
history, whose commands may reference the previous source baseline.

Save & Publish waits for source writes, cooks and validates an immutable runtime
generation, then adopts that exact generation. Atmosphere changes affect the
generation hash even if no cell content changed. A failed cook or adoption keeps
the previous runtime look. **Published** comparison follows the successfully
adopted runtime profile; a source save alone does not advance it.

Atmosphere edits do not rebuild terrain/vegetation authoring previews. Publication
uses the existing full cooker. The renderer creates device-specific lookup
textures at runtime; they are not source assets or per-cell payloads.

Gameplay preview takes an isolated snapshot of the draft at its authored startup
phase. Leaving Gameplay returns to the prior authoring preview. As in the standalone
game, startup time remains fixed in this slice; automatic game time is not wired
up yet. Authoring Play is available to inspect the complete day cycle.

## Implemented contracts

- `world::atmosphere`: validated profile, explicit units/colors and deterministic
  elevation-based evaluation.
- `world_db`: revisioned source writes and bounded metadata loading.
- `world_cook`: validation, hashing and immutable publication.
- `yarra-atmosphere`: shared Bevy atmosphere, light, ambient, exposure and haze adapter.
- `engine`: world-profile selection, generation adoption, rebasing and ownership.
- `app_editor::atmosphere_authoring`: window, drafts, temporary transport, history,
  recovery, conflicts and saving.

Source/runtime schemas are 24/20; the recovery journal is version 13 (12 remains
readable). This follows
the repository's explicit format-reset policy, not a general legacy migration
framework. Back up existing databases before updating their format.

## Verification

The workspace suite covers evaluation/wrapping/shallow sun paths, invalid profiles,
revision conflicts, recovery, shared undo, preview/source isolation, atmosphere-only
publication, world changes preserving session time, and study lighting ownership.
A native Metal smoke test checked clear/day/sunrise/haze output with terrain and
grass; temporary scrubbing; numeric edit and undo; Save & Publish; saved profile
reopening; and World → Vegetation → World restoration. The standalone game also
loaded the published profile and restored it after entering and leaving an interior.
Night checks also cover moon/exposure interpolation, daylight preservation,
selection of the moon for grass lighting/shadows, saved night parameters, exposure
locking and a first Vegetation-study visit from night. New Vegetation studies and the Animation workspace use stable daylight;
saved vegetation-study lighting remains independent. This is functional validation, not a
sustained performance benchmark.
