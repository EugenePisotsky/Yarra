#!/usr/bin/env python3
"""Capture the editor-only ground treatment with identical grass across material variants."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MODES = {"original": "Original ground", "darkened": "Darken only", "understory": "Detailed understory", "coverage": "Coverage mask"}


def sparse_source(source):
    # Edit the selected population in our serialized StudyDocument, with a strict
    # match check rather than silently changing another population or all species.
    key = re.search(r'\bpopulation: "([^"]+)"', source).group(1)
    pattern = r'(\bkey: "' + re.escape(key) + r'",[\s\S]*?density_per_square_meter: )([0-9.]+)'
    matches = list(re.finditer(pattern, source))
    if len(matches) != 1:
        raise ValueError("Cannot identify one selected population density in source study")
    match = matches[0]
    density = float(match.group(2))
    return source[:match.start(2)] + str(density * 0.25) + source[match.end(2):]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=ROOT / "content/vegetation/distance-01.ron")
    parser.add_argument("--output", type=Path, required=True, help="New, empty experiment directory")
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory to preserve earlier experiments")
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "yarra-app-editor"], cwd=ROOT, check=True)
    output.mkdir(parents=True, exist_ok=True)
    source = args.source.read_text()
    if len(re.findall(r'\bwind: \(\s*enabled: (true|false)', source)) != 1:
        raise ValueError("Source must contain one study wind transport")
    wind_source = re.sub(r'(\bwind: \(\s*enabled: )(true|false)', r'\g<1>true', source)
    sources = {"dense": wind_source, "sparse": sparse_source(wind_source)}
    close, changes = re.subn(r'camera: \([\s\S]*?\n    \),',
        'camera: (yaw: 0.0, pitch: 72.0, distance: 1.85, target: (0.0, 0.08, 0.0), fov: 50.0),',
        wind_source, count=1)
    if changes != 1:
        raise ValueError("Cannot prepare close-range study camera")
    sources["close"] = close
    for name, text in sources.items():
        (output / f"{name}-source.ron").write_text(text)
    specs = [
        ("edge-rest", "Edge · wind off", "dense", "overhead", True, None),
        ("edge-0", "Edge · wind 0 s", "dense", "overhead", True, 0),
        ("edge-1", "Edge · wind 1 s", "dense", "overhead", True, 1),
        ("dense-top", "Dense · top down", "dense", "top", False, 0),
        ("dense-close", "Dense · close ground detail", "close", None, False, 0),
        ("dense-low", "Dense · low angle", "dense", "low", False, 0),
        ("sparse-top", "Quarter density · top down", "sparse", "top", False, 0),
    ]
    frames = []
    for key, label, density, camera, edge, phase in specs:
        frame = {"key": key, "label": label, "density": density, "images": {}, "diagnostics": {}}
        mask_hashes = set()
        counts = set()
        for mode in MODES:
            folder = output / f"{key}-{mode}"
            command = [sys.executable, str(ROOT / "tools/vegetation_study.py"), "capture", "--no-build",
                "--load", str(output / f"{density}-source.ron"),
                "--ground", mode, "--field", "16", "--edge" if edge else "--no-edge",
                "--no-character", "--output", str(folder)]
            command += ["--no-wind"] if phase is None else ["--time", str(phase)]
            if camera:
                command += ["--camera", camera]
            print(f"Capturing {label}: {MODES[mode]}", flush=True)
            subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
            treatment = (folder / "ground-treatment.txt").read_text()
            diagnostics = (folder / "diagnostics.txt").read_text()
            mask_hashes.add(re.search(r'Coverage hash FNV1a: (\w+)', treatment).group(1))
            emitted = tuple(map(int, re.search(r'emitted_instances: \[([^]]+)\]', diagnostics).group(1).replace(',', ' ').split()))
            counts.add(emitted)
            frame["images"][mode] = f"{folder.name}/viewport.png"
            frame["diagnostics"][mode] = f"{folder.name}/ground-treatment.txt"
        if len(mask_hashes) != 1 or len(counts) != 1:
            raise ValueError(f"Ground variants changed source coverage or grass counts: {key}")
        frame["coverage_hash"] = mask_hashes.pop()
        frame["emitted"] = list(counts.pop())
        frames.append(frame)
    # The edge source must keep its coverage through both wind phases and wind-off.
    if len({f["coverage_hash"] for f in frames if f["key"].startswith("edge-")}) != 1:
        raise ValueError("Wind changed static source coverage")
    if len({f["coverage_hash"] for f in frames if f["key"].startswith("dense-")}) != 1:
        raise ValueError("Camera changed static source coverage")
    data = {"modes": MODES, "frames": frames, "source": str(args.source.resolve()),
        "source_sha256": hashlib.sha256(source.encode()).hexdigest()}
    (output / "comparison.json").write_text(json.dumps(data, indent=2) + "\n")
    template = (ROOT / "tools/grass_ground_study.html").read_text()
    (output / "comparison.html").write_text(template.replace("/*GROUND_DATA*/", json.dumps(data).replace("<", "\\u003c")))
    print(f"All material variants retain identical grass counts and masks.\nComparison: {output / 'comparison.html'}")


if __name__ == "__main__":
    main()
