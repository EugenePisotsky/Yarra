#!/usr/bin/env python3
"""Local, uncommitted catalog experiments through the production vegetation study renderer.

Prepare variants from a captured study, then capture one fixed view at a time. No catalog saves,
publication, density increases, topology-section increases, or extra render passes are involved.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def replace(text, key, value):
    pattern = rf"(\b{re.escape(key)}: )[^,\n]+"
    result, count = re.subn(pattern, lambda m: m[1] + str(value), text)
    if count != 1:
        raise ValueError(f"Expected one {key}, found {count}")
    return result


def edit_block(text, marker, changes):
    start = text.index(marker)
    opening = text.index("(", start + len(marker))
    depth = 1
    end = opening + 1
    while depth:
        depth += (text[end] == "(") - (text[end] == ")")
        end += 1
    body = text[opening:end]
    for key, value in changes.items():
        body = replace(body, key, value)
    return text[:opening] + body + text[end:]


def variant(text, name):
    if name in LENGTH_VARIANTS:
        return length_variant(text, name)
    text = edit_block(text, "wind:", {"enabled": "false", "time": "0.0"})
    start = text.index('key: "short_split_fill_ribbon"')
    end = text.index("        populations:", start)
    species = text[start:end]
    if "arches" in name:
        species = edit_block(species, "curve_variant_a:", {
            "tip_tilt_radians": 1.10, "root_tangent_radians": 0.18,
            "tip_tangent_radians": 1.75, "root_handle_length": 0.45,
            "tip_handle_length": 0.25,
        })
        species = edit_block(species, "curve_variant_b:", {
            "tip_tilt_radians": 1.34, "root_tangent_radians": 0.45,
            "tip_tangent_radians": 1.95, "root_handle_length": 0.55,
            "tip_handle_length": 0.32,
        })
        species = replace(species, "maximum_lateral_curve", 0.08)
        species = replace(species, "pair_spread_radians", 0.45)
    if name in ("pairs_arches", "budget_pairs_arches"):
        species = replace(species, "pair_below_height", 0.69)
    if name == "canopy_arches":
        species = replace(species, "distribution_bias", 0.25)
    text = text[:start] + species + text[end:]
    start = text.index('key: "short_split_fill",', text.index("        populations:"))
    end = text.index("        assemblages:", start)
    population = text[start:end]
    if name in ("distributed", "small_groups", "distributed_arches", "pairs_arches", "budget_pairs_arches", "canopy_arches"):
        population = replace(population, "root_attraction", 0.05)
    if name in ("small_groups", "distributed_arches", "pairs_arches", "budget_pairs_arches", "canopy_arches"):
        population = replace(population, "spacing", 0.6)
    if name in ("distributed_arches", "pairs_arches", "budget_pairs_arches", "canopy_arches"):
        population = replace(population, "shared_group_weight", 1.5)
        population = replace(population, "radial_weight", 0.75)
        population = replace(population, "random_weight", 1.5)
    if name == "budget_pairs_arches":
        density = float(re.search(r"density_per_square_meter: ([^,]+)", population)[1])
        population = replace(population, "density_per_square_meter", density * 0.75)
    return text[:start] + population + text[end:]


VARIANTS = ("baseline", "distributed", "small_groups", "arches", "distributed_arches", "pairs_arches", "budget_pairs_arches", "canopy_arches")
LENGTH_VARIANTS = ("length_baseline", "taller_mix", "longer_135", "longer_165", "longer_mix", "varied_arcs", "layered_arcs", "layered_species", "layered_budget")


def length_variant(text, name):
    """Test length distribution, then varied arches, without adding roots or sections."""
    text = edit_block(text, "wind:", {"enabled": "false", "time": "0.0"})
    if name in ("layered_species", "layered_budget"):
        text = layered_species_variant(text)
        if name == "layered_budget":
            start = text.index('key: "short_split_fill",', text.index("        populations:"))
            end = text.index("        assemblages:", start)
            population = text[start:end]
            density = float(re.search(r"density_per_square_meter: ([^,]+)", population)[1])
            population = replace(population, "density_per_square_meter", round(density * 44 / 45, 6))
            text = text[:start] + population + text[end:]
        return text
    start = text.index('key: "short_split_fill_ribbon"')
    end = text.index("        populations:", start)
    species = text[start:end]
    scale = {"longer_135": 1.35, "longer_165": 1.65, "longer_mix": 1.35, "varied_arcs": 1.35, "layered_arcs": 1.35}.get(name, 1.0)
    if scale != 1.0:
        for key in ("minimum_height", "maximum_height", "pair_below_height", "maximum_horizontal_reach"):
            value = float(re.search(rf"\b{key}: ([^,]+)", species)[1])
            species = replace(species, key, round(value * scale, 6))
    if name in ("taller_mix", "longer_mix", "varied_arcs", "layered_arcs"):
        species = replace(species, "distribution_bias", 0.35)
    if name == "varied_arcs":
        species = edit_block(species, "curve_variant_a:", {
            "tip_tilt_radians": 0.68, "root_tangent_radians": 0.05,
            "tip_tangent_radians": 2.1, "root_handle_length": 0.55,
            "tip_handle_length": 0.45,
        })
        species = edit_block(species, "curve_variant_b:", {
            "tip_tilt_radians": 1.48, "root_tangent_radians": 0.45,
            "tip_tangent_radians": 2.55, "root_handle_length": 0.75,
            "tip_handle_length": 0.48,
        })
        species = edit_block(species, "group_response:", {
            "height_coherence": 0.2, "silhouette_coherence": 0.12,
        })
    if name == "layered_arcs":
        species = edit_block(species, "curve_variant_a:", {
            "tip_tilt_radians": 1.05, "root_tangent_radians": 0.08,
            "tip_tangent_radians": 1.95, "root_handle_length": 0.46,
            "tip_handle_length": 0.22,
        })
        species = edit_block(species, "curve_variant_b:", {
            "tip_tilt_radians": 1.5, "root_tangent_radians": 0.65,
            "tip_tangent_radians": 2.5, "root_handle_length": 0.40,
            "tip_handle_length": 0.22,
        })
        species = edit_block(species, "group_response:", {
            "height_coherence": 0.2, "silhouette_coherence": 0.12,
        })
    return text[:start] + species + text[end:]


def layered_species_variant(text):
    """Use the existing weighted species choice at each root, not extra populations or roots."""
    key = text.index('key: "short_split_fill_ribbon"')
    start = text.rfind("            (\n                id:", 0, key)
    end = text.index("        ],\n        populations:", key)
    if start < 0:
        raise ValueError("Expected the short-grass species as the final species entry")
    original = text[start:end]
    profiles = [
        # id, key, weight, length bounds, bias, two complete resting curves
        (4, "short_split_fill_ribbon", 0.65, (0.38, 1.02), 0.2,
         (1.36, 0.45, 2.25, 0.40, 0.22), (1.50, 0.75, 2.50, 0.48, 0.25)),
        (5, "study_middle_arch", 0.25, (0.30, 0.85), 0.1,
         (1.02, 0.10, 1.60, 0.50, 0.25), (1.25, 0.40, 2.10, 0.55, 0.32)),
        (6, "study_high_arch", 0.10, (0.24, 0.62), -0.1,
         (0.75, 0.05, 1.90, 0.38, 0.20), (1.00, 0.15, 2.20, 0.45, 0.25)),
    ]
    entries, choices = [], []
    for identity, name, weight, bounds, bias, curve_a, curve_b in profiles:
        uuid = "((" + ", ".join([str(identity)] * 16) + "))"
        species, count = re.subn(r"id: \(\([^\n]+?\)\)", "id: " + uuid, original)
        if count != 1:
            raise ValueError("Expected one species id in the prototype")
        species = replace(species, "key", '"' + name + '"')
        species = edit_block(species, "bounds:", {
            "minimum_height": bounds[0], "maximum_height": bounds[1],
            "maximum_horizontal_reach": bounds[1],
        })
        species = edit_block(species, "height:", {
            "distribution_bias": bias, "pair_below_height": bounds[1],
        })
        for marker, values in [("curve_variant_a:", curve_a), ("curve_variant_b:", curve_b)]:
            species = edit_block(species, marker, dict(zip((
                "tip_tilt_radians", "root_tangent_radians", "tip_tangent_radians",
                "root_handle_length", "tip_handle_length",
            ), values)))
        species = edit_block(species, "group_response:", {
            "height_coherence": 0.2, "silhouette_coherence": 0.12,
        })
        entries.append(species)
        choices.append(f"                    (species: {uuid}, weight: {weight}),")
    text = text[:start] + "".join(entries) + text[end:]
    start = text.index('key: "short_split_fill",', text.index("        populations:"))
    end = text.index("        assemblages:", start)
    population, count = re.subn(
        r"species: \[.*?\n                \],",
        "species: [\n" + "\n".join(choices) + "\n                ],",
        text[start:end], count=1, flags=re.DOTALL,
    )
    if count != 1:
        raise ValueError("Expected one selected population species list")
    return text[:start] + population + text[end:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "capture"))
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--suite", choices=("density", "length"), default="density")
    parser.add_argument("--variants", nargs="+", choices=VARIANTS + LENGTH_VARIANTS)
    parser.add_argument("--view", choices=("top_down", "top_down_close", "edge_1", "bottom_straight", "lod"), default="top_down")
    parser.add_argument("--ground", choices=("neutral", "meadow"), default="neutral")
    args = parser.parse_args()
    output = args.output.resolve()
    if args.action == "prepare":
        if args.baseline is None:
            parser.error("prepare requires --baseline study.ron")
        output.mkdir(parents=True, exist_ok=False)
        original = args.baseline.read_text()
        variants = LENGTH_VARIANTS if args.suite == "length" else VARIANTS
        for name in variants:
            (output / f"{name}.ron").write_text(variant(original, name))
        shaders = [ROOT / "assets/shaders" / name for name in (
            "vegetation_blade.wgsl", "vegetation_debug_draw.wgsl", "vegetation_debug_compute.wgsl")]
        (output / "shaders").mkdir()
        for path in shaders:
            (output / "shaders" / path.name).write_bytes(path.read_bytes())
        (output / "manifest.json").write_text(json.dumps({
            "baseline": str(args.baseline.resolve()),
            "baseline_sha256": hashlib.sha256(original.encode()).hexdigest(),
            "shaders": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in shaders},
            "constraints": (
                "No additional roots, widths, sections, LOD thresholds or shaders. Length bounds/distribution change; pairing threshold and reach follow length. varied_arcs and layered_arcs widen curves and reduce height/silhouette coherence. layered_species uses three weighted profiles in one population; layered_budget also reduces density from 45 to 44 (proportionally). Wind disabled. All must pass measured geometry and cost checks."
                if args.suite == "length" else
                "No increased root density, width/height bounds, section counts or LOD thresholds; wind disabled. Pairs variants change pairing threshold; budget_pairs_arches also reduces root density by 25%; canopy changes height distribution within existing bounds. All must pass geometry and cost checks."
            ),
            "variants": variants,
        }, indent=2) + "\n")
        print(output)
        return
    manifest = json.loads((output / "manifest.json").read_text())
    for name, expected in manifest["shaders"].items():
        actual = hashlib.sha256((ROOT / "assets/shaders" / name).read_bytes()).hexdigest()
        if actual != expected:
            raise ValueError(f"Shader {name} differs from this experiment's manifest; restore its archived shader before capturing")
    for name in args.variants or manifest["variants"]:
        if name not in manifest["variants"]:
            raise ValueError(f"Variant {name} is not in this experiment")
        destination = output / f"{name}-{args.view}-{args.ground}"
        print(f"Capturing {name} / {args.view} / {args.ground}", flush=True)
        subprocess.run([
            sys.executable, str(ROOT / "tools/vegetation_study.py"), "capture", "--no-build",
            "--load", str(output / f"{name}.ron"), "--select-reference", args.view,
            "--ground", args.ground, "--no-character", "--zoom", "1", "--time", "0",
            "--output", str(destination),
        ], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
