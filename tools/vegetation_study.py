#!/usr/bin/env python3
"""Build/open/capture the actual Yarra vegetation workspace. No alternate scene implementation."""
import argparse
import datetime
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".editor" / "vegetation"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["open", "capture", "inspect"])
    parser.add_argument("--reference", action="append", type=Path, default=[], help="Import local PNG/JPEG, retaining the full-resolution source; repeat for multiple references")
    parser.add_argument("--camera", choices=["low", "overhead", "top", "scale"])
    parser.add_argument("--select-reference", help="Select a loaded reference by name (e.g. edge_1) and restore its camera and stage")
    parser.add_argument("--zoom", type=float, help="Shared image magnification, 1 to 6")
    parser.add_argument("--inspector", action="store_true", help="Open the floating inspector")
    parser.add_argument("--picker", action="store_true", help="Open the floating reference picker")
    parser.add_argument("--character", action="store_true", help="Show the real game character (default for new studies)")
    parser.add_argument("--no-character", action="store_true", help="Hide the scale character")
    parser.add_argument("--ruler", action="store_true", help="Show the separate 2 m ruler")
    parser.add_argument("--field", type=int, choices=[4, 16, 64, 128], help="Field width in metres; overrides reference preset")
    parser.add_argument("--ground", choices=["neutral", "meadow", "dried"], help="Study ground material")
    edge = parser.add_mutually_exclusive_group()
    edge.add_argument("--edge", dest="edge", action="store_const", const=True, default=None, help="Show a grass boundary with clear foreground")
    edge.add_argument("--no-edge", dest="edge", action="store_const", const=False, help="Fill the complete field")
    parser.add_argument("--time", type=float, help="Exact frozen wind phase in seconds")
    parser.add_argument("--load", type=Path, help="Replay study.ron as an unsaved catalog draft")
    parser.add_argument("--output", type=Path, help="New capture directory, or existing directory to inspect")
    parser.add_argument("--no-build", action="store_true", help="Use the existing debug binary")
    args = parser.parse_args()
    if args.action == "inspect":
        captures = sorted((LOCAL / "captures").glob("*/study.ron"), key=lambda p: p.stat().st_mtime)
        folder = args.output or (captures[-1].parent if captures else None)
        if folder is None:
            parser.error("No captures found; run capture first or supply --output")
        for name in ["viewport.png", "editor.png", "study.ron", "diagnostics.txt", "run.log"]:
            path = folder.resolve() / name
            print(f"{name}: {path}" if path.exists() else f"Missing: {path}")
        diagnostic = folder / "diagnostics.txt"
        if diagnostic.exists():
            print(diagnostic.read_text())
        return 0
    binary = ROOT / "target" / "debug" / "yarra-app-editor"
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "yarra-app-editor"], cwd=ROOT, check=True)
    if not binary.exists():
        parser.error(f"Build the editor first: {binary}")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    output = (args.output or LOCAL / "captures" / stamp).resolve()
    if args.action == "capture" and output.exists() and any(output.iterdir()):
        parser.error(f"Capture directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    command = [str(binary), "--vegetation-study"]
    if args.character:
        command += ["--study-character"]
    if args.no_character:
        command += ["--study-no-character"]
    if args.ruler:
        command += ["--study-ruler"]
    if args.field is not None:
        command += ["--study-field", str(args.field)]
    if args.ground:
        command += ["--study-ground", args.ground]
    if args.edge is not None:
        command += ["--study-edge" if args.edge else "--study-no-edge"]
    for path in args.reference:
        command += ["--study-reference", str(path.expanduser().resolve(strict=True))]
    if args.camera:
        command += ["--study-camera", args.camera]
    if args.select_reference:
        command += ["--study-select-reference", args.select_reference]
    if args.zoom is not None:
        command += ["--study-zoom", str(args.zoom)]
    if args.inspector:
        command += ["--study-inspector"]
    if args.picker:
        command += ["--study-picker"]
    if args.time is not None:
        command += ["--study-time", str(args.time)]
    if args.load:
        command += ["--study-load", str(args.load.resolve(strict=True))]
    if args.action == "capture":
        command += ["--study-capture", str(output), "--study-exit-after-capture"]
    env = os.environ.copy()
    revision = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    env["YARRA_STUDY_BUILD"] = f"{revision}{' + working tree changes' if dirty else ''}; binary mtime {binary.stat().st_mtime_ns}"
    log_path = output / "run.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        if args.action == "open":
            print(f"Vegetation editor PID {process.pid}\nLog: {log_path}")
            return 0
        try:
            code = process.wait(timeout=90)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            print(f"Capture timed out. Log: {log_path}", file=sys.stderr)
            return 1
    missing = [name for name in ["viewport.png", "editor.png", "study.ron", "diagnostics.txt"] if not (output / name).is_file()]
    log_text = log_path.read_text()
    render_errors = any(" ERROR " in line or "panicked at" in line for line in log_text.splitlines())
    if code or missing or render_errors:
        print(log_text[-10000:], file=sys.stderr)
        print(f"Capture failed (exit={code}, missing={missing}, render_errors={render_errors}). Log: {log_path}", file=sys.stderr)
        return 1
    print(f"Viewport: {output / 'viewport.png'}\nEditor: {output / 'editor.png'}\nReplay: {output / 'study.ron'}\nDiagnostics: {output / 'diagnostics.txt'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
