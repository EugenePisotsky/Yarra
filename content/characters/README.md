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
| `layers` | plain MakeHuman, a plain game material, MPFB's subsurface skin and procedural eyes |
| `skins` | the CC0 skins on one face |
| `expressions` | ARKit face units combined into common expressions |
| `faces` | faces drawn from 37 face sliders (`face_variants.py`, seeded) |

## The base

- Base mesh (MakeHuman's hm08): 19,158 vertices, 13,380 of them the body, the rest helper
  geometry (eyes, teeth, tongue, joint markers) that the export removes.
- Targets: macro sliders (gender, age, weight, muscle, height, proportions) and 270 face and head
  targets; `faceunits01` adds the 52 ARKit face shapes, `visemes01` 22 mouth shapes for speech.
- Packs: `makehuman_system_assets` (default skins, eyes, eyebrows, eyelashes, teeth, tongue, hair,
  clothes, alternative topologies), the CC0 community skins of `skins01`–`03`, `eyebrows01`,
  `eyelashes01`.
