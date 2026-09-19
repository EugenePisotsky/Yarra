"""Summarize the archived short GPU runs using the shared profiling parser."""

import argparse
import json
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools"))
from grass_profile_report import fields, quantile, timestamp


def summarize(path, utc_offset):
    lines = path.read_text().splitlines()
    events = [fields(line) for line in lines if "GRASS_PROFILE " in line]
    start = next((event for event in events if event.get("event") == "measure_start"), None)
    end = next((event for event in events if event.get("event") == "complete"), None)
    if not start or not end:
        raise ValueError(f"Incomplete measurement: {path}")
    begin, finish = int(start["unix_ms"]) / 1000, int(end["unix_ms"]) / 1000
    gpu, presentation = [], []
    previous_stamp = previous_packet = None
    for line in lines:
        if "metal-HUD: " not in line:
            continue
        stamp = timestamp(line, utc_offset)
        packet = line.split("metal-HUD: ", 1)[1]
        if packet == previous_packet:
            continue
        previous_packet = packet
        # Exclude the first packet that straddles the measurement boundary.
        # Packets still overlap each other; these are correlated HUD samples.
        if previous_stamp is not None and previous_stamp >= begin and stamp <= finish:
            try:
                values = list(map(float, packet.split(",")))
            except ValueError:
                continue
            if len(values) >= 5 and (len(values) - 3) % 2 == 0:
                gpu.extend(values[4::2])
                presentation.extend(values[3::2])
        previous_stamp = stamp
    samples = [event for event in events if event.get("event") == "sample"]
    audits = [fields(line) for line in lines if "RENDER_AUDIT " in line]
    audits = [event for event in audits if begin <= int(event["unix_ms"]) / 1000 <= finish]
    return {
        "fps": int(end["frames"]) / float(end["measured_s"]),
        "max_update_ms": max(float(sample["update_max_ms"]) for sample in samples),
        "late_updates": sum(int(sample["late_updates"]) for sample in samples),
        "gpu_mean": statistics.mean(gpu) if gpu else None,
        "gpu_p95": quantile(gpu, 0.95),
        "presentation_p95": quantile(presentation, 0.95),
        "focused": all(sample["focused"] == "true" for sample in samples),
        "thermal": sorted({event.get("thermal") for event in audits}),
        "last_audit": audits[-1] if audits else None,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log_directory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--utc-offset", type=int, default=10800,
                        help="Log timezone offset in seconds; archived runs use UTC+3")
    args = parser.parse_args()
    paths = sorted(args.log_directory.glob("*.log"))
    if not paths:
        parser.error("No .log files in the input directory")
    result = {path.stem: summarize(path, args.utc_offset) for path in paths}
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Summarized {len(result)} runs into {args.output}")
