#!/usr/bin/env python3
"""Compare Xcode's Export Encoder Counters CSV files (not Metal HUD logs).

Usage: python3 tools/summarize_gpu_counters.py reference.csv candidate.csv --output result.json
Times are summed encoder work in a replay, not live frame latency or energy.
"""

import argparse
import csv
import json
import math
import re
from collections import Counter, defaultdict
from pathlib import Path


def read_counters(path):
    with path.open(newline="", encoding="utf-8-sig") as source:
        reader = csv.reader(source)
        names = Counter()
        header = []
        for name in next(reader):
            names[name] += 1
            header.append(name if names[name] == 1 else f"{name} [{names[name]}]")
        if not {"Encoder Label", "GPU Time"}.issubset(header):
            raise ValueError(f"{path}: expected an encoder-counter export")
        result = []
        for line, row in enumerate(reader, 2):
            if not row:
                continue
            # Xcode 26 can export unquoted decimal commas in percentage cells.
            # Repair only rows with surplus fields; never collapse a valid CSV.
            if len(row) > len(header):
                repaired = []
                for value in row:
                    if (re.fullmatch(r"\d+%", value) and repaired
                            and re.fullmatch(r"-?\d+", repaired[-1])):
                        repaired[-1] += "." + value
                    else:
                        repaired.append(value)
                row = repaired
            if len(row) != len(header):
                raise ValueError(f"{path}:{line}: {len(row)} cells, expected {len(header)}")
            result.append(dict(zip(header, row)))
    if not result:
        raise ValueError(f"{path}: no encoder measurements")
    return result


def summarize(path):
    rows = read_counters(path)
    passes = defaultdict(float)
    for row in rows:
        duration = float(row["GPU Time"])
        if not math.isfinite(duration) or duration < 0:
            raise ValueError(f"{path}: missing/invalid GPU time for {row['Encoder Label']}")
        passes[row["Encoder Label"]] += duration / 1_000_000
    return {
        "source": str(path),
        "encoder_count": len(rows),
        "summed_encoder_ms": sum(passes.values()),
        "passes_ms": dict(sorted(passes.items(), key=lambda item: -item[1])),
        # Keep all counters and duplicate columns for investigating a selected pass.
        "encoders": rows,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    reference, candidate = summarize(args.reference), summarize(args.candidate)
    print("Xcode replay encoder time; not live frame latency. Match GPU performance state.")
    print("| Pass | Reference ms | Candidate ms | Change |")
    print("| --- | ---: | ---: | ---: |")
    ref_passes, candidate_passes = reference["passes_ms"], candidate["passes_ms"]
    entries = [("All encoders (sum)", reference["summed_encoder_ms"], candidate["summed_encoder_ms"])]
    for label in sorted(ref_passes.keys() | candidate_passes.keys(),
                        key=lambda label: -ref_passes.get(label, 0)):
        entries.append((label, ref_passes.get(label, 0), candidate_passes.get(label, 0)))
    for label, before, after in entries:
        change = f"{100 * (after / before - 1):+.1f}%" if before else "n/a"
        print(f"| {label} | {before:.4f} | {after:.4f} | {change} |")
    if args.output:
        args.output.write_text(json.dumps({"reference": reference, "candidate": candidate}, indent=2) + "\n")


if __name__ == "__main__":
    main()
