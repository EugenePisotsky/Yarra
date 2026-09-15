#!/usr/bin/env python3
"""Field-scale visual trials through the real editor renderer, with isolated assets.

No publication, live-asset edits, triangle shadows, or performance collection.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

from grass_density_experiment import edit_block, replace

ROOT = Path(__file__).resolve().parents[1]
VARIANTS = ("baseline", "structure", "canopy", "understory", "overlap", "joined",
            "unthinned", "fullshape", "filled", "palette")
VIEWS = {
    "overhead": (0.0, 48.0, 13.0, 0.5),
    "away": (180.0, 10.0, 4.0, 0.9),
    "toward": (0.0, 10.0, 4.0, 0.9),
    "top-close": (180.0, 75.0, 6.0, 0.5),
    "top-far": (180.0, 75.0, 10.0, 0.5),
}

# Authored canopy approximation, NOT measured visibility or a reconstruction of Yotei's shader.
# Rest-space depth makes this profile independent of the camera and of wind phase.
CANOPY = """
fn field_curve_height(curve: vec4<f32>, tip_y: f32, t: f32) -> f32 {
    let s = 1.0 - t;
    return max(0.0, 3.0*s*s*t*curve.y + 3.0*s*t*t*(tip_y-curve.w) + t*t*t*tip_y);
}
fn field_crown(curve: vec4<f32>, tip_y: f32) -> f32 {
    return max(0.06, max(tip_y, max(field_curve_height(curve,tip_y,0.25),
        max(field_curve_height(curve,tip_y,0.5),field_curve_height(curve,tip_y,0.75)))));
}
fn field_exposure(instance: ProceduralInstance, profile: Species, t: f32, companion: bool) -> f32 {
    let seed = hash32(instance.geometry.w & 0x00ffffffu);
    let group = u32(round(unpack2x16unorm(bitcast<u32>(instance.root_clump.w)).x * 65535.0));
    let group_height = random01(group ^ 0x52dce729u);
    let height_t = pow(mix(random01(seed ^ 0xa511e9b3u), group_height, profile.group_response.x), exp2(-2.0*profile.height_packing.x));
    let height = mix(profile.bounds.x,profile.bounds.y,height_t) * select(1.0,0.8,companion);
    let shape = mix(random01(seed ^ 0x27d4eb2fu),random01(group ^ 0x7b7d159cu),profile.group_response.y);
    let curve = mix(profile.curve_variant_a,profile.curve_variant_b,shape);
    let tip_y = cos(mix(profile.shape.x,profile.shape.y,shape));
    let rest_y = height * field_curve_height(curve,tip_y,t);
    let group_curve = mix(profile.curve_variant_a,profile.curve_variant_b,random01(group ^ 0x7b7d159cu));
    let group_tip_y = cos(mix(profile.shape.x,profile.shape.y,random01(group ^ 0x7b7d159cu)));
    let canopy_height = mix(profile.bounds.x,profile.bounds.y,pow(group_height,exp2(-2.0*profile.height_packing.x))) * field_crown(group_curve,group_tip_y);
    return smoothstep(0.06,0.85,rest_y/max(canopy_height,0.06));
}
"""


def canopy_shader(draw):
    anchor = "fn surface_normal_from_debug_instance"
    assert anchor in draw
    draw = draw.replace(anchor, CANOPY + "\n" + anchor, 1)
    draw = draw.replace("struct VertexOutput {", "struct VertexOutput {\n    @location(10) canopy_exposure: f32,", 1)
    old = "let color = mix(profile.root_color.xyz, profile.tip_color_height.xyz, shading_t) * (1.0 + variation);"
    new = """let canopy_exposure = field_exposure(instance, profile, shading_t, paired_companion);
    let color = mix(profile.root_color.xyz, profile.tip_color_height.xyz, shading_t) * (1.0 + variation);"""
    assert old in draw
    draw = draw.replace(old, new, 1)
    draw = draw.replace("output.color = color;", "output.color = color;\n    output.canopy_exposure = canopy_exposure;", 1)
    draw = draw.replace("let diffuse_energy = min(exposed_sun_peak * camera.lighting.x, 1.20);", "let diffuse_energy = min(exposed_sun_peak * camera.lighting.x, 1.20)\n        * mix(0.48, 1.0, input.canopy_exposure);", 1)
    draw = draw.replace("let foliage_ambient = input.color", "let foliage_ambient = input.color\n        * mix(0.38, 1.0, input.canopy_exposure)", 1)
    return draw


def source_variant(text, variant, view, phase):
    yaw, pitch, distance, target_y = VIEWS[view]
    text = re.sub(r"camera: \([\s\S]*?\n    \),", f"camera: (yaw: {yaw}, pitch: {pitch}, distance: {distance}, target: (0.0, {target_y}, 0.0), fov: 45.0),", text, count=1)
    text = edit_block(text, "wind:", {"enabled": "true" if phase is not None else "false", "time": phase or 0.0})
    text = re.sub(r"render_size: \([^)]*\)", "render_size: (1920, 1080)", text)
    text = replace(text, "msaa_samples", 4)
    text = replace(text, "patch_size", 64.0)
    text = replace(text, "show_character", "true")
    if variant == "understory":
        text = re.sub(r"\bground: Meadow", "ground: UnderstoryStudy", text, count=1)
    if variant in ("joined", "unthinned", "fullshape", "filled", "palette"):
        text = re.sub(r"\bground: Meadow", "ground: CanopyGroundStudy", text, count=1)
    if variant in ("unthinned", "fullshape", "filled", "palette"):
        text = replace(text, "density_mode", "FullReference")
    if variant == "baseline":
        return text
    first = text.index('key: "short_split_fill_ribbon"')
    end = text.index("        populations:", first)
    species = text[first:end]
    # Shared silhouettes and heights form masses, with remaining per-leaf variation.
    overlap = variant in ("overlap", "joined", "unthinned", "fullshape", "filled", "palette")
    species = re.sub(r"height_coherence: [^,]+", f"height_coherence: {0.5 if overlap else 0.68}", species)
    species = re.sub(r"silhouette_coherence: [^,]+", f"silhouette_coherence: {0.5 if overlap else 0.65}", species)
    species = re.sub(r"clump_color_variation: [^,]+", f"clump_color_variation: {0.12 if overlap else 0.25}", species)
    if variant == "palette":
        species = re.sub(r"root_color: \([^)]*\)", "root_color: (0.048, 0.092, 0.033)", species)
        species = re.sub(r"tip_color: \([^)]*\)", "tip_color: (0.11, 0.235, 0.058)", species)
    text = text[:first] + species + text[end:]
    start = text.index('key: "short_split_fill",', text.index("        populations:"))
    end = text.index("        assemblages:", start)
    pop = text[start:end]
    changes = {"spacing": 0.85, "root_attraction": 0.26, "boundary_softness": 0.24,
                       "shared_group_weight": 2.2, "radial_weight": 1.1,
                       "random_weight": 0.55, "flow_weight": 0.35}
    if overlap:
        changes.update(spacing=0.6, root_attraction=0.04, boundary_softness=0.40,
                       shared_group_weight=1.6, radial_weight=0.7, random_weight=0.8,
                       flow_weight=0.4)
    for key, value in changes.items():
        pop = replace(pop, key, value)
    if variant in ("filled", "palette"):
        pop = replace(pop, "density_per_square_meter", 72.0)
    return text[:start] + pop + text[end:]


def prepare(folder, source):
    folder.mkdir(parents=True, exist_ok=False)
    (folder / "source.ron").write_text(source)
    for variant in VARIANTS:
        assets = folder / variant / "assets"
        assets.mkdir(parents=True)
        for path in (ROOT / "assets").iterdir():
            if path.name != "shaders":
                (assets / path.name).symlink_to(path, target_is_directory=path.is_dir())
        shutil.copytree(ROOT / "assets/shaders", assets / "shaders")
        if variant in ("canopy", "understory"):
            shader = assets / "shaders/vegetation_debug_draw.wgsl"
            shader.write_text(canopy_shader(shader.read_text()))
        if variant in ("fullshape", "filled", "palette"):
            shader = assets / "shaders/vegetation_debug_compute.wgsl"
            code = shader.read_text()
            begin = code.index("    var lod = 0u;", code.index("fn evaluate_candidate("))
            end = code.index("    let bin =", begin)
            code = code[:begin] + "    let lod = 0u;\n    let lod_morph = 1.0;\n" + code[end:]
            shader.write_text(code)


def capture(folder, variant, view, phase):
    if variant in ("fullshape", "filled", "palette") and view in ("away", "toward"):
        raise ValueError("Full-shape control is limited to overhead field views to avoid high-bin clipping")
    if variant in ("filled", "palette") and view == "overhead":
        raise ValueError("72-root full-shape control requires the close/far top views; the wider overview clips")
    work = folder / variant
    name = view + ("" if phase is None else f"-wind-{phase:g}")
    output = work / name
    output.mkdir(exist_ok=False)
    source = source_variant((folder / "source.ron").read_text(), variant, view, phase)
    doc = output / "input.ron"
    doc.write_text(source)
    command = [str(ROOT / "target/debug/yarra-app-editor"), "--vegetation-study", "--study-load", str(doc),
               "--study-field", "64", "--study-no-edge", "--study-blade-bands", "off",
               "--study-capture", str(output), "--study-exit-after-capture"]
    (output / "run.json").write_text(json.dumps({"command":command,"variant":variant,"view":view,"wind":phase},indent=2))
    env = dict(os.environ, MTL_HUD_ENABLED="0", MTL_HUD_LOG_ENABLED="0")
    with (output / "run.log").open("w") as log:
        process = subprocess.Popen(command, cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            code = process.wait(timeout=90)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=10)
            raise RuntimeError(f"Capture timed out: {output}")
    errors = [line for line in (output / "run.log").read_text().splitlines() if " ERROR " in line or "panicked at" in line]
    if code or errors or not (output / "viewport.png").exists():
        raise RuntimeError(f"Capture failed: {output}\n" + "\n".join(errors[-12:]))
    diagnostics = (output / "diagnostics.txt").read_text()
    dropped = re.search(r"capacity_dropped_instances: \[([^]]+)\]", diagnostics)
    if not dropped or any(int(n) for n in re.findall(r"\d+", dropped[1])):
        (output / "invalid.txt").write_text("Capacity clipping: this image cannot establish visual quality.")
        raise RuntimeError(f"Appearance control clipped: {output}")
    print(output / "viewport.png", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "capture", "report"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=ROOT / "content/vegetation/distance-01.ron")
    parser.add_argument("--variants", nargs="+", choices=VARIANTS, default=list(VARIANTS))
    parser.add_argument("--views", nargs="+", choices=VIEWS, default=["overhead", "away"])
    parser.add_argument("--phase", type=float)
    parser.add_argument("--reference", type=Path, nargs="+")
    args = parser.parse_args()
    folder = args.output.resolve()
    if args.action == "prepare":
        prepare(folder, args.source.read_text())
    elif args.action == "report":
        frames = []
        for variant in VARIANTS:
            for frame in sorted((folder / variant).glob("*/run.json")):
                data = json.loads(frame.read_text())
                if (frame.parent / "viewport.png").exists() and not (frame.parent / "invalid.txt").exists():
                    frames.append({"variant": variant, "view": data["view"], "wind": data["wind"],
                                   "image": str((frame.parent / "viewport.png").relative_to(folder))})
        references = []
        if args.reference:
            for i, path in enumerate(args.reference):
                name = f"reference-{i + 1}" + path.suffix.lower()
                shutil.copy2(path, folder / name)
                references.append(name)
        (folder / "comparison.json").write_text(json.dumps({"frames": frames, "references": references}, indent=2))
        shutil.copy2(ROOT / "tools/grass_field_study.html", folder / "comparison.html")
        print(folder / "comparison.html")
    else:
        for view in args.views:
            for variant in args.variants:
                capture(folder, variant, view, args.phase)


if __name__ == "__main__":
    main()
