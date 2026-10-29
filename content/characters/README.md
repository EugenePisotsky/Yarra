# Characters

Yarra's people are built from MPFB, the MakeHuman plugin for Blender: one base mesh for every
body, its sliders (targets) for body types and faces, and garments fitted to that mesh so they
follow every body. Everything it outputs is ours: MPFB's assets are CC0 and its licence claims
nothing over exports. Its code is GPL-3.0-or-later; it runs in Blender, and the game's Rust is
written from the file formats, not ported from it.

The goals (October 2026): two body types per gender, faces from presets and sliders, ARKit
expressions for facial capture later, and clothing and armour worn piece by piece in slots.

## Where it lives

| What | Where | Tracked |
| --- | --- | --- |
| MPFB version and asset packs, pinned by hash | `content/characters/mpfb.toml` | Git |
| Setup and build scripts | `content/characters/scripts/` | Git |
| The Blender profile with MPFB and the packs | `.tools/blender/` | Local only, rebuilt by setup |
| Downloaded packs | `.tools/downloads/<id>/` | Local only |
| Built characters | `assets/local/characters/<name>/` | Local only |
| Review renders | `tmp/characters/` | Local only |

## Setup

```sh
python3 content/characters/scripts/setup_blender.py        # --replace rebuilds the profile
```

It downloads the pinned packs (about 620 MB, kept between runs), makes the profile at
`.tools/blender`, installs MPFB from extensions.blender.org through Blender's extension client
(refusing any version or archive but the pinned one) and unpacks the packs into MPFB's data
folder. The user's own Blender profile and add-ons are left alone. Scripts run in the profile:

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender \
  -b --python-exit-code 1 --python content/characters/scripts/<script>.py -- ARGS
```

Blender-side scripts reach MPFB's services through `scripts/mpfb_services.py`
(`service("humanservice", "HumanService")`): Blender loads an extension under a package path known
only at run time.

To move to a newer MPFB, check its release notes, update `[mpfb]` in `mpfb.toml` to the version
and archive hash the index lists, and rebuild the profile.

## Reviews

Looks are judged on contact sheets of fixed views under one light rig, rendered from review specs
in `reviews/` (a preset, views, variants merged over it; see `scripts/render_views.py`):

```sh
content/characters/scripts/render_reviews.sh              # all; or: render_reviews.sh faces skins
python3 content/characters/scripts/face_variants.py       # regenerate reviews/faces.json
```

Each review lands in `tmp/characters/<name>/`, a folder of PNGs per variant and `sheet.png`.
Cycles renders them: EEVEE shows MPFB's procedural eyes as mirrors. A face picked from the faces
sheet is kept by copying its `targets` into a preset in `presets/`.

| Review | Shows |
| --- | --- |
| `bodies` | the bought body the game plays now beside MPFB bodies (fitted, wrapped, sliders only), in clay at one scale |
| `outfits` | MPFB's female outfits and shoes on the fitted body |
| `brows` | the modelled eyebrows and eyelashes beside MPFB's card ones |
| `layers` | plain MakeHuman, a plain game material, MPFB's subsurface skin and procedural eyes |
| `skins` | the CC0 skins on one face |
| `expressions` | ARKit face units combined into common expressions |
| `faces` | faces drawn from 37 face sliders (`face_variants.py`, seeded) |

## Face sliders

`face_sliders.json` lists the face sliders characters are shaped with, by feature group: each a
pair of MPFB targets (the first for negative values, none for a slider that only goes one way;
an `l-` name moves both sides) and the view that shows it best. It began as 100 candidates from
MPFB's 350 or so face and head targets; the audit renders every slider at both ends beside the
preset's face, and the list keeps the sliders worth offering, with their limits.

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/slider_audit.py -- \
  content/characters/face_sliders.json tmp/characters/sliders            # or name GROUPs
for m in tmp/characters/sliders/*/manifest.json; do
  uv run -q --with pillow python content/characters/scripts/sheet.py $(dirname $m) --columns 2 --height 300
done
```

The human is built once (about 18 minutes for all 100); each slider's targets go on over the
preset's and the eyes, brows, lashes, teeth and hair are fitted to the face again, as the game
will do when a slider moves. Fine features (lids, folds, nostrils, lips) are judged in close views
of the eyes, nose or mouth: whole-face views show them too small.

## Fitting a body to another model

`fit_body.py` finds the MPFB sliders that make its body match another body's surface and writes
them into a preset: only slider values, nothing of the other mesh.

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/fit_body.py -- \
  assets/local/characters/female_main/female_main_locomotion.glb \
  content/characters/presets/probe_female.json content/characters/presets/fitted_body.json \
  tmp/characters/fit_points.npz
uv run --with numpy --with pillow python content/characters/scripts/overlay.py \
  tmp/characters/fit_points.npz tmp/characters/fit_overlay.png
```

It fits six macro sliders and about 60 region sliders (torso, hips, legs, arms, neck, head size
and posture) by point-to-plane ICP with Levenberg-Marquardt steps, about 20 s. MPFB's arms are
turned to the other body's at shoulder and elbow as part of the fit, since the two hold them
differently; hands are left out. `fitted_body.json` is the game's bought body (female_main)
fitted this way (October 28): mean distance head 3.4, torso 5.1, legs 6.5, arms 6.6 mm. What
sliders cannot give is surface definition (collarbones, abdomen, knees): a normal map or a
sculpted target's job. The overlay draws both bodies as fitted (the other red, MPFB cyan).

`wrap_body.py` takes the rest: it moves the fitted body onto the other body's surface and writes
the offsets as one target over the preset's sliders, in MPFB's rest pose.

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/wrap_body.py -- \
  assets/local/characters/female_main/female_main_locomotion.glb \
  content/characters/presets/fitted_body.json assets/local/characters/fitted_body/wrap.npz
```

The preset's body is posed as in the fit, each body vertex is drawn to the nearest point of the
other surface through a smooth field of offsets, loosened step by step, and then set on it. The
head, hands and feet stay MPFB's, blended in by skin weight at the neck, wrists and ankles: the
face sliders, eyes and fingers belong to them. MakeHuman's mesh spends most of its body vertices
there, so about 4,800 carry the torso, arms and legs. The rig's joint cubes for the shoulders,
elbows, hips and knees move to the other rig's joints, the points its clips turn the limbs about
(each within 1.5 mm over female_main's clips), and every other helper follows the body near it.
A preset names the result as `"wrap"`; `build_human` applies it and refits the rig and the parts.
Being the bought body's shape, the wrap stays in `assets/local` beside it.

On `fitted_body.json` (October 28) the wrap moves vertices 6.6 mm on average (95% within 15 mm)
and brings in what the sliders missed: the deltoids, the back, collarbones, elbows and knees.
The shoulder joints moved 37 mm out, the knees 82 mm up, the hips 18 mm and the elbows 10 mm. In
the idle the shoulders' width matches the bought body's (0.42-0.45 m across the upper arms,
against 0.39-0.40 before).

## In the game

A preset is exported rigged, on MPFB's game-engine skeleton (53 bones; body, garments and hair
skinned by MPFB's weights; a wrapped body's joints where the wrap put them), with the bought
body's Idle, Walk and Jog retargeted onto it:

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/export_glb.py -- \
  content/characters/presets/fitted_body.json assets/local/characters/fitted_body/fitted_body.glb \
  assets/local/characters/female_main/female_main_locomotion.glb
cargo run --release -p yarra-app-game -- --character-probe presentations/female/mpfb_fitted
```

The character catalog (`assets/catalogs/character_presentations.catalog.ron`) holds that glb as
the profile `presentations/female/mpfb_fitted`. `--character-probe` stands a profile ahead of the
player, facing the camera and playing its idle; the editor's Animation workspace plays its walk
and jog. The glb's clips come from bought animations, so it stays in `assets/local` like
female_main's.

The fitted character costs about what female_main does: 23,461 vertices and 39,832 triangles
against 19,183 and 36,004, using MPFB's card eyebrows and eyelashes (124 and 250 vertices; the
modelled mindfront ones the look probe wore are 20,130 and 16,992). It draws in 9 parts
to female_main's one, and its 10 textures take 142 MiB as the game loads them now, uncompressed
and without mipmaps (female_main has none).

The export bakes the sliders into the mesh, removes MPFB's helpers and the skin under clothes, and
uses one plain PBR material per part; textures with holes (the ponytail's, the eyes', the
teeth's) are cut out at half alpha (glTF MASK), the rest stay opaque. With clips it prints the
measures the catalog's `feet` takes: the ankle and toe joints' lowest heights over the clips and
the sole's reach behind the ankle and ahead of the toes.

`retarget.py` turns each game-engine bone by one source bone's world-space turn, measured from a
reference pose: the target posed so its limbs point where the source's do at rest, since MPFB holds
its arms lower and its fingers curled. Only turns carry over, with the pelvis's rise and sway
scaled by hip height; the target's own bone lengths place its joints. Side by side in the same
clips (October 28) the poses match, fists and head turns included. On the wrapped body, whose
joints are the source's, the ankle's lowest point walking is 7 mm below the source's (MPFB's
ankle sits lower in its foot) and the toes' matches it.

## The base

- Base mesh (MakeHuman's hm08): 19,158 vertices, 13,380 of them the body, the rest helper
  geometry (eyes, teeth, tongue, joint markers) that the export removes.
- Targets: macro sliders (gender, age, weight, muscle, height, proportions) and 270 face and head
  targets; `faceunits01` adds the 52 ARKit face shapes, `visemes01` 22 mouth shapes for speech.
- Packs: `makehuman_system_assets` (default skins, eyes, eyebrows, eyelashes, teeth, tongue, hair,
  clothes, alternative topologies), the CC0 community skins of `skins01`–`03`, `eyebrows01`,
  `eyelashes01`.
