#!/usr/bin/env python3
"""Matched local Metal HUD smoke check; whole-editor GPU duration, not isolated grass timing.

Runs only the process it starts, with wind playing. Supplied draw and blade shaders temporarily
replace the current ones and are restored on exit. Run sequentially with no other GPU work/builds.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SHADER_NAMES = ("vegetation_debug_draw.wgsl", "vegetation_blade.wgsl")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--study", required=True, type=Path)
    parser.add_argument("--shaders", required=True, type=Path, help="Directory containing both archived shaders")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--seconds", type=int, default=25, choices=range(10, 61))
    parser.add_argument("--pause-editor-pid", type=int, help="Temporarily suspend a live review editor, preserving its unsaved state")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    study = args.study.read_text()
    assert study.count("        enabled: false,") == 1
    study = study.replace("        enabled: false,", "        enabled: true,", 1)
    (output / "study.ron").write_text(study)
    previous = {name: (ROOT / "assets/shaders" / name).read_bytes() for name in SHADER_NAMES}
    shaders = {name: (args.shaders / name).read_bytes() for name in SHADER_NAMES}
    env = dict(os.environ, MTL_HUD_ENABLED="1", MTL_HUD_LOG_ENABLED="1")
    process = None
    paused_editor = None
    changed_shaders = {name: shader for name, shader in shaders.items() if shader != previous[name]}
    try:
        if args.pause_editor_pid is not None:
            identity = subprocess.check_output([
                "ps", "-p", str(args.pause_editor_pid), "-o", "comm=",
            ], text=True).strip()
            if identity != str(ROOT / "target/debug/yarra-app-editor"):
                raise ValueError("Only this repository's debug editor may be paused")
            state = subprocess.check_output([
                "ps", "-p", str(args.pause_editor_pid), "-o", "stat=",
            ], text=True).strip()
            if "T" not in state:
                os.kill(args.pause_editor_pid, signal.SIGSTOP)
                paused_editor = args.pause_editor_pid
        for name, shader in changed_shaders.items():
            (ROOT / "assets/shaders" / name).write_bytes(shader)
        with (output / "run.log").open("w") as log:
            process = subprocess.Popen([
                str(ROOT / "target/debug/yarra-app-editor"), "--vegetation-study",
                "--study-load", str(output / "study.ron"), "--study-play",
            ], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
            for elapsed in range(args.seconds):
                try:
                    code = process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    if (elapsed + 1) % 5 == 0:
                        print(f"{output.name}: {elapsed + 1}/{args.seconds}s", flush=True)
                else:
                    raise RuntimeError(f"Editor exited before benchmark completed: {code}")
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        try:
            for name in changed_shaders:
                (ROOT / "assets/shaders" / name).write_bytes(previous[name])
        finally:
            if paused_editor is not None:
                try:
                    os.kill(paused_editor, signal.SIGCONT)
                except ProcessLookupError:
                    pass
    packets = []
    previous_payload = None
    text = (output / "run.log").read_text()
    if "panicked at" in text or " ERROR " in text:
        raise RuntimeError(f"Render error in {output / 'run.log'}")
    for line in text.splitlines():
        if "metal-HUD: " not in line:
            continue
        payload = line.split("metal-HUD: ", 1)[1].strip()
        if payload == previous_payload:
            continue
        previous_payload = payload
        values = [float(x) for x in payload.split(",")]
        if len(values) < 5 or (len(values) - 3) % 2:
            raise ValueError("Unexpected Metal HUD packet")
        packets.append(values)
    # Drop initial reports containing asset loading / pipeline compilation. Report order is
    # not wall time, and equal pairs within a packet are preserved.
    samples = [v for packet in packets[5:] for v in packet[4::2]]
    result = {
        "scope": "Whole-editor Metal HUD GPU duration with wind playing; not isolated GPU busy time or phone performance",
        "study": str(args.study.resolve()), "seconds": args.seconds,
        "paused_review_editor_pid": args.pause_editor_pid,
        "shader_sha256": {name: hashlib.sha256(shader).hexdigest() for name, shader in shaders.items()},
        "packets": len(packets), "discarded_startup_packets": 5, "samples": len(samples),
        "gpu_mean_ms": statistics.mean(samples) if samples else None,
        "gpu_median_ms": statistics.median(samples) if samples else None,
        "gpu_p95_ms": sorted(samples)[int((len(samples) - 1) * .95)] if samples else None,
    }
    (output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
    if not samples:
        raise RuntimeError("No usable GPU timing samples; inspect run.log")


if __name__ == "__main__":
    main()
