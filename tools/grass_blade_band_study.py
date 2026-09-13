#!/usr/bin/env python3
"""Capture optional blade bands against identical grass and ground, including frozen wind poses."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from grass_ground_study import sparse_source

ROOT = Path(__file__).resolve().parents[1]
MODES = {"off": "Bands off", "subtle": "Subtle bands", "medium": "Medium bands", "mask": "Band mask"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=ROOT / "content/vegetation/distance-01.ron")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--previous", type=Path, help="Earlier report directory using the identical scene matrix; include its Medium captures")
    parser.add_argument("--motion-frames", type=int, default=0, help="Additional fixed-blade mask captures at 0.25 s intervals (0–40)")
    args = parser.parse_args()
    if not 0 <= args.motion_frames <= 40:
        parser.error("--motion-frames must be between 0 and 40")
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory to preserve previous captures")
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "yarra-app-editor"], cwd=ROOT, check=True)
    output.mkdir(parents=True, exist_ok=True)
    source = args.source.read_text()
    source, count = re.subn(r'(\bwind: \(\s*enabled: )(true|false)', r'\g<1>true', source)
    if count != 1:
        raise ValueError("Expected one wind transport")
    close, count = re.subn(r'camera: \([\s\S]*?\n    \),',
        'camera: (yaw: 0.0, pitch: 72.0, distance: 1.85, target: (0.0, 0.08, 0.0), fov: 50.0),', source, count=1)
    if count != 1:
        raise ValueError("Expected one camera")
    sources = {"dense": source, "close": close, "sparse": sparse_source(close)}
    for key, value in sources.items():
        (output / f"{key}-source.ron").write_text(value)
    specs = [
        ("dense-rest", "Dense close · wind off", "close", None, False, None),
        ("dense-0", "Dense close · wind 0 s", "close", None, False, 0),
        ("dense-half", "Dense close · wind 0.5 s", "close", None, False, .5),
        ("dense-1", "Dense close · wind 1 s", "close", None, False, 1),
        ("dense-low", "Dense · low angle", "dense", "low", False, 0),
        ("sparse", "Quarter density · close", "sparse", None, False, 0),
        ("edge", "Grass boundary · overhead", "dense", "overhead", True, 0),
    ]
    previous = None
    if args.previous:
        previous = json.loads((args.previous / "comparison.json").read_text())
        previous = {frame["key"]: frame for frame in previous["frames"]}
    frames = []
    for key, label, source_key, camera, edge, phase in specs:
        frame = {"key": key, "label": label, "images": {}, "diagnostics": {}}
        counts, masks = set(), set()
        for mode in MODES:
            folder = output / f"{key}-{mode}"
            command = [sys.executable, str(ROOT / "tools/vegetation_study.py"), "capture", "--no-build",
                "--load", str(output / f"{source_key}-source.ron"), "--ground", "understory", "--field", "16",
                "--blade-bands", mode, "--edge" if edge else "--no-edge", "--no-character", "--output", str(folder)]
            command += ["--no-wind"] if phase is None else ["--time", str(phase)]
            if camera:
                command += ["--camera", camera]
            print(f"Capturing {label}: {MODES[mode]}", flush=True)
            subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
            diagnostics = (folder / "diagnostics.txt").read_text()
            counts.add(tuple(map(int, re.search(r'emitted_instances: \[([^]]+)\]', diagnostics).group(1).replace(',', ' ').split())))
            masks.add(hashlib.sha256((folder / "ground-coverage.png").read_bytes()).hexdigest())
            frame["images"][mode] = f"{folder.name}/viewport.png"
            frame["diagnostics"][mode] = f"{folder.name}/diagnostics.txt"
        if len(counts) != 1 or len(masks) != 1:
            raise ValueError(f"Band setting changed grass count or ground coverage: {key}")
        frame["emitted"] = list(counts.pop())
        frame["coverage_hash"] = masks.pop()
        if previous is not None:
            old = previous[key]
            if old["emitted"] != frame["emitted"] or old["coverage_hash"] != frame["coverage_hash"]:
                raise ValueError(f"Previous report has different geometry or ground coverage: {key}")
            folder = output / "previous" / key
            folder.mkdir(parents=True, exist_ok=True)
            shutil.copy2(args.previous / old["images"]["medium"], folder / "viewport.png")
            shutil.copy2(args.previous / old["diagnostics"]["medium"], folder / "diagnostics.txt")
            frame["images"]["previous"] = f"previous/{key}/viewport.png"
            frame["diagnostics"]["previous"] = f"previous/{key}/diagnostics.txt"
        frames.append(frame)
    modes = ({"previous": "Previous Medium", **MODES} if previous is not None else MODES)
    data = {"frames": frames, "modes": modes, "source": str(args.source.resolve())}
    motion = []
    for index in range(args.motion_frames):
        phase = index * .25
        folder = output / f"motion-{index:02}"
        print(f"Capturing fixed blades, moving marks: {phase} s", flush=True)
        subprocess.run([sys.executable, str(ROOT / "tools/vegetation_study.py"), "capture", "--no-build",
            "--load", str(output / "close-source.ron"), "--ground", "understory", "--field", "16",
            "--blade-bands", "motion-mask", "--no-edge", "--no-character", "--time", str(phase),
            "--output", str(folder)], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
        diagnostics = (folder / "diagnostics.txt").read_text()
        emitted = list(map(int, re.search(r'emitted_instances: \[([^]]+)\]', diagnostics).group(1).replace(',', ' ').split()))
        dense = next(frame for frame in frames if frame["key"] == "dense-0")
        coverage_hash = hashlib.sha256((folder / "ground-coverage.png").read_bytes()).hexdigest()
        if emitted != dense["emitted"] or coverage_hash != dense["coverage_hash"]:
            raise ValueError(f"Motion diagnostic changed grass count or ground coverage at {phase} s")
        motion.append({"time": phase, "image": f"{folder.name}/viewport.png"})
    data["motion"] = motion
    (output / "comparison.json").write_text(json.dumps(data, indent=2) + "\n")
    template = (ROOT / "tools/grass_blade_band_study.html").read_text()
    (output / "comparison.html").write_text(template.replace("/*BAND_DATA*/", json.dumps(data).replace("<", "\\u003c")))
    print(f"Verified identical emitted units and ground masks.\n{output / 'comparison.html'}")


if __name__ == "__main__":
    main()
