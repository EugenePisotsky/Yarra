"""Renders each face slider at both ends of its range on a preset's face: the review that decides
which sliders the game offers and how far they go.

blender -b --python-exit-code 1 --python slider_audit.py -- SLIDERS.json OUT_DIR [GROUP ...]

SLIDERS (content/characters/face_sliders.json) names the preset and the sliders by group. A slider
is a pair of MPFB targets, the first taken for negative values, none for a slider that only goes
one way, and the range players may move it in; an "l-" name moves both sides. The human is built
once. For each end of each slider's range its targets go on as shape keys over the preset's, the parts (eyes, brows, lashes, teeth, hair) are
fitted to the face again, the slider's view is rendered and the keys come off. The camera stays
where the preset's face put it, so every change shows against one frame. Writes
OUT_DIR/<group>/<slider>/{low,preset,high}.png and a manifest per group for sheet.py; preset is
the preset's own face, as is an end at zero. GROUPs, if given, limit the run to those
groups.
"""
import json
import shutil
import sys
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import ROOT, build_human, service, sides
from render_views import FRONT, add_camera, framing, light_rig, render_settings, render_view

TargetService = service("targetservice", "TargetService")
ClothesService = service("clothesservice", "ClothesService")

# Each slider's sheet cell: its low end, the preset's face, its high end.
COLUMNS = ("low", "preset", "high")


def set_slider(basemesh, slider, value):
    """Shape keys putting the slider at `value` (-1 to 1) over the preset."""
    negative, positive = slider["targets"]
    keys = []

    for target in sides(negative if value < 0 else positive):
        path = TargetService.target_full_path(target)
        keys.append(TargetService.load_target(basemesh, path, weight=abs(value), name=f"audit-{target}"))

    return keys


def refit(basemesh, parts):
    for part in parts:
        if part != basemesh:
            ClothesService.fit_clothes_to_human(part, basemesh)


def main():
    args = sys.argv[sys.argv.index("--") + 1 :]
    spec_path, out, only = Path(args[0]), Path(args[1]), set(args[2:])
    spec = json.loads(spec_path.read_text())
    preset = json.loads((ROOT / spec["preset"]).read_text())
    render_settings("CYCLES")
    basemesh, parts, eyes = build_human(preset)
    placement = framing(parts, eyes)
    light_rig(FRONT)
    camera = add_camera()
    views = {s["view"] for g in spec["groups"] for s in g["sliders"]}
    neutral = {view: out / "neutral" / f"{view}.png" for view in views}

    for view, path in neutral.items():
        render_view(camera, view, placement, path)

    for group in spec["groups"]:
        if only and group["name"] not in only:
            continue

        folder = out / group["name"]
        variants = []

        for slider in group["sliders"]:
            target = folder / slider["label"]
            target.mkdir(parents=True, exist_ok=True)
            shutil.copy(neutral[slider["view"]], target / "preset.png")

            for end, value in zip(("low", "high"), slider["range"]):
                path = target / f"{end}.png"

                if value == 0:
                    shutil.copy(neutral[slider["view"]], path)
                    continue

                keys = set_slider(basemesh, slider, value)
                refit(basemesh, parts)
                render_view(camera, slider["view"], placement, path)

                for key in keys:
                    basemesh.shape_key_remove(key)

                refit(basemesh, parts)

            (negative, positive), (low, high) = slider["targets"], slider["range"]
            note = f"{low:+.1f} {negative or 'none'} | {high:+.1f} {positive}"
            variants.append({"label": slider["label"], "note": note})
            print(f"AUDITED {group['name']} / {slider['label']}")

        manifest = {"views": list(COLUMNS), "variants": variants}
        (folder / "manifest.json").write_text(json.dumps(manifest, indent=2))

    print("AUDIT_DONE")


main()
