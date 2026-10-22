"""GPU time per pass and frame from a Metal System Trace's interval table.

Export the table first (docs/WORKFLOWS.md, Metal System Trace):

    xcrun xctrace export --input a.trace --output a-intervals.xml \
      --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'

then `python3 tools/summarize_gpu_intervals.py a-intervals.xml [b-intervals.xml]`. Each pass is
an encoder label, summed over the vertex, fragment and compute channels and divided by the frames
the trace holds (the passes run once a frame: `--frames-by`). The times follow the GPU's clock;
compare two traces by each pass's share of a reference pass in the same trace (`--reference`).
"""

import argparse
import collections
import re
import xml.etree.ElementTree as ET

PROCESS = "yarra-app-game"


def intervals(path):
    """(pass label, channel, duration ns) of the game's intervals."""
    seen = {}

    def value(element):
        ref = element.get("ref")
        if ref is not None:
            return seen.get(ref, ("", ""))
        for child in element.iter():
            if child.get("id") is not None:
                seen[child.get("id")] = (child.get("fmt", ""), child.text or "")
        return (element.get("fmt", ""), element.text or "")

    columns = []
    for _, element in ET.iterparse(path, events=("end",)):
        if element.tag == "schema":
            columns = [c.findtext("mnemonic") for c in element.findall("col")]
        elif element.tag == "row":
            row = dict(zip(columns, (value(c) for c in element)))
            element.clear()
            if PROCESS not in row.get("process", ("", ""))[0]:
                continue
            label = row.get("event-label", ("", ""))[0].split(":")[0].strip()
            label = re.sub(r"Command Buffer \d+", "Command Buffer", label)
            yield label, row.get("channel-name", ("", ""))[0], int(row["duration"][1] or 0)


def summarize(path, frames_by):
    totals = collections.Counter()
    counts = collections.Counter()
    for label, channel, duration in intervals(path):
        totals[label] += duration
        if channel == "Fragment":
            counts[label] += 1
    frames = max(counts[frames_by], 1)
    return {label: ns / frames / 1e6 for label, ns in totals.items()}, frames


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("traces", nargs="+")
    parser.add_argument("--frames-by", default="temporal tone map to output")
    parser.add_argument("--reference", default="main_opaque_pass_3d")
    parser.add_argument("--top", type=int, default=30)
    args = parser.parse_args()

    tables = [summarize(path, args.frames_by) for path in args.traces]
    for path, (_, frames) in zip(args.traces, tables):
        print(f"{path}: {frames} frames")
    order = sorted(tables[0][0], key=lambda label: -tables[0][0][label])
    for label in order[: args.top]:
        cells = []
        for passes, _ in tables:
            ms = passes.get(label, 0.0)
            share = ms / passes.get(args.reference, float("nan"))
            cells.append(f"{ms:7.2f} ms {share:6.2f}x")
        print(f"{label[:48]:48} " + "  ".join(cells))


if __name__ == "__main__":
    main()
