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
an `l-` name moves both sides), the range players may move it in, and the view that shows it
best. Sliders sharing a `choice` exclude each other (face shape, ear shape: the creator offers
one and its strength); `detail` marks those hard to see at a face's full framing, which the
creator keeps in a section of their own. The audit renders every slider at both ends of its range
beside the preset's face.

The list (October 30) keeps 93 of 100 candidates from MPFB's 350 or so face and head targets.
Subtle is not redundant: a slider is dropped only when another moves the same vertices the same
way (nose base and nose length, 0.96 alike; oval and round head, 0.88; ear wing and ears sticking
out; septum and nostril angle; cupid's bow width and upper lip width; bag height and cheek height;
ear thickness and round ears). Most others look believable to between half and two thirds of
their full range, where the limits stand; pointed ears go all the way, for elves.

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

Beside the glb the export writes its kit (`kit.py`, `fitted_body.kit.json`, 4 MB), which the game
reshapes the face and fits the parts with: the base mesh as exported (every vertex, helpers too),
each face slider's offsets at both ends, and every part's MHCLO fitting (three base-mesh
vertices, weights and an offset scaled by the base mesh's size along each axis), in glTF's
space. Each exported vertex names its base-mesh or part vertex in a `_SOURCE_VERTEX` attribute.
The export checks that fitting from the kit puts every part where MPFB did (within 0.0003 mm);
the body matches exactly because the export bakes its shape keys before MPFB removes the helpers
(MPFB's own bake rebuilds every key through copies and moves vertices up to 0.1 mm).

The character catalog (`assets/catalogs/character_presentations.catalog.ron`) holds that glb and
its kit as the profile `presentations/female/mpfb_fitted`; the engine builds the character's
meshes from the kit when it spawns and when its `CharacterAppearance` changes
(`engine/src/character_kit.rs`).

Faces are made in the editor's Character workspace: with a kit model's profile chosen, the face
panel offers the brow's style and colour, makeup, its sliders by group (choices as a pick and a
strength, details folded away), a random face around the model's own, and keeps faces by name in
`assets/characters/faces/<name>.face.ron` (`engine/src/character_face.rs`). The game's probe
wears one with `--character-face NAME`. `--character-probe` stands a profile ahead of the
player, facing the camera and playing its idle; the editor's Character workspace plays its walk
and jog beside any other profile. `--character-portrait DIR` then saves fixed views of the probe,
its face to the sun, and exits. The glb's clips come from bought animations, so it stays in `assets/local` like
female_main's.

Spine and neck do not take the source's turns alone: they aim at where the next joint belongs, the
point of the source's spine at that joint's rest height, because the two rigs' spine bones differ
in number and length. Measured against the source over the clips, the head's world position differs
by 1-8 mm (RMS, means removed), and its side-to-side sway relative to the pelvis is 6.1 cm in the walk against the
source's 6.0 (8.4 with turns alone) and 7.4 in the jog against 7.7 (11.0).

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

A preset's `"skin"` (a skin file in `content/characters/skins/`) gives the head its own material
and layout. MakeHuman's atlas spends its head island on the scalp and neck, leaving a mouth 66
texels of 2K; laid out as a scan is (`head_layout.py`: a thin-plate spline through the skin file's
pairs of island points and scan pixels), a mouth has about 560 of the head's 4K. The bought scan and
what is baked from it stay out of the repository; the skin file holds only coordinates and tone.

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/head_mesh.py -- \
  content/characters/presets/fitted_body.json assets/local/characters/head_mesh.npz
uv run --with numpy --with scipy --with pillow --with tifffile python \
  content/characters/scripts/skin_bake.py content/characters/skins/face07.json \
  ~/Downloads/texturingxyz__multichannelfaces_07 \
  .tools/blender/extensions/.user/blender_org/mpfb/data/skins/young_caucasian_female/young_lightskinned_female_diffuse.png \
  assets/local/characters/head_mesh.npz assets/local/characters/fitted_body/skin \
  --cache tmp/characters/skin07/clean
```

`skin_bake.py` takes the scan's brows and lashes out of its colour and height, sets its level to
the skin file's tone, and writes `skin/*.ktx2` beside the glb, with mipmaps (about 6 minutes). Head
and body meet in one colour: near their seam the head takes the body's colour there, by real
distance from it. The insides of the mouth and eye sockets are dark, and the lips' and lids' inner
edges shaded, since no shadow map resolves them. The export gives the glb stand-in images and
rewrites its materials to name those files (`skin_export.py`). Head colour and normal are 4K (BC7 on desktop, 1 byte a texel: about 21 MiB
each with mipmaps), its occlusion-roughness 2K (5 MiB). The body's colour is the atlas at the same tone.

`fit_parts.py` replaces the parts MakeHuman's own do not make good enough. Its eye is one cut-out
texture on one surface, which reads as a doll's. A preset's `"eye_model"` puts three surfaces of
revolution in its place, fitted by MakeHuman's eye vertices so they follow the same sliders: an
opaque sclera and a recessed iris, and over them a wet layer with the cornea's bulge, which adds
reflections and takes its alpha from the shade the lids throw and a soft ring at the limbus. The
iris has no normal map: its relief turns each fibre bright or dark by the light's direction. Its
lashes are thick cut-out cards; strands take their place, 130 per eye, tapered ribbons rooted on the
lid's edge, fanned, curled and gathered in loose clumps, over a soft dark strip along each lid that
reads as the line of their roots. Each is bound to the triangle of skin at its root so lid sliders
carry it, and they draw with soft edges. A vertex shader keeps them at least a pixel wide, faded by
as much, since thinner ones fall between the samples at the game's render scale and break into specks
(`shaders/lighting/lash_strand.wesl`). The eye textures come from a bought pack
([licences](../../docs/ASSET_LICENSES.md)), kept out of the repository like the scan:

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/eye_mesh.py -- \
  ~/Downloads/Eye3dModels/eye.blend assets/local/characters/fitted_body/eye_mesh.npz
uv run --with numpy --with scipy --with pillow --with tifffile python \
  content/characters/scripts/eye_bake.py ~/Downloads/EyePack 5 \
  assets/local/characters/fitted_body/eye_mesh.npz assets/local/characters/fitted_body/skin
```

The number picks the pack's iris. The eyes take 3,460 vertices against MakeHuman's 1,064 and one
blended draw, the lashes 2,256 vertices and 1,728 triangles against 250 vertices and another blended
draw; the textures take 6 MiB.

Brows and makeup are not parts: the skin paints them as it draws (`engine/src/character_skin.rs`,
`shaders/lighting/skin_layers.wesl`), from masks in the head's layout, with a colour and strength
each from the character's face file. A brow therefore follows every face slider, and its hairs
resolve into soft coverage in the mipmaps. A preset leaves out MakeHuman's (`"eyebrows": ""`).

```sh
uv run --with numpy --with scipy --with pillow --with tifffile python \
  content/characters/scripts/skin_layers_bake.py content/characters/skins/face07.json \
  ~/Downloads/eyebrows assets/local/characters/head_mesh.npz assets/local/characters/fitted_body/skin
```

It takes about a minute. Each brow of the bought pack is unbent through its plane's UVs and taken apart into
its centre line and thickness. The line is laid along the crest of the head's own brow ridge (level
from the nose, falling towards the temple), rising from a lower inner end to a peak two-thirds out
and keeping a third of its drawn arch, by arc length over the forehead; the hairs lie about it at a
little over half their drawn thickness, both sides from the one brow. Run the export afterwards,
which copies `skin/skin_layers.json` into the kit. A brow mask is 3072x640 (2.6 MiB with mipmaps, only the
worn one loaded), the makeup mask 2048x2048 (5.3 MiB): eyeshadow, liner, blush and lipstick. The
hairline mask (2048x2048, 5.3 MiB) is the worn hair's cover of the head: found by projecting the hair's
cards onto the skin (not the ears, which the skin file names), solid under them so no skin shows
between them, then fine strands and a soft haze over the skin in front of them, painted in the hair's
colour. Their cut-out edge then reads as hair. It belongs to one hairstyle: a hairstyle choice will need
a mask each. Each head costs three texture samples in the G-buffer, on its own pixels only, and has
its own material.

A character's hair is a style its kit offers, not a part of its glb: a preset wearing styles has
`"hair": ""`, and `content/characters/hairs/female_hair_12.json` names the styles and their labels, in
the order the Face panel offers them. Each is a folder in `assets/local/characters/hairs/`, bought
([licences](../../docs/ASSET_LICENSES.md)) and kept out of the repository. `hair_pack.py` reads the
pack's meshes once; `hair_fit.py` makes a style of one hair:

```sh
BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
  --python-exit-code 1 --python content/characters/scripts/hair_pack.py -- \
  ~/Downloads/hairstyles/Blender+2.83.blend assets/local/characters/hairs/pack.npz
uv run --with numpy --with scipy --with pillow --with tifffile python \
  content/characters/scripts/hair_fit.py assets/local/characters/hairs/pack.npz hair8 \
  <hair8's folder from hair8.rar> content/characters/skins/face07.json \
  assets/local/characters/head_mesh.npz assets/local/characters/hairs
```

About 2 minutes a style. It lays the hair on the head, by a scale and shift over the skull and then the
smooth field of what is left between the pack's head and ours (6.4 mm apart on average), binds each
vertex to the triangle of skin nearest it so sliders carry it, and bakes the hairline mask. Use each
hair's own textures from its archive: a card's UVs fit no other. Run the export afterwards: it lists
the styles built in the kit.

The game loads only the style worn (`engine/src/character_hair.rs`), keeping the old one until the new
is ready, about 11 MiB of textures, 5 MiB of hairline mask and its mesh. Its mesh hangs off the head
joint, unskinned, so long hair does not follow the shoulders and passes through them. The textures are
grey, each character's hair colour tinting them, and the cards cut out at a threshold that changes per
pixel and per frame, which the temporal upscaler averages into soft edges; one fixed threshold at half
resolution reads as straw. A face's `hair_style` is a label, or `None` for a bare head.

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
