"""GPU time per pass and frame from a Metal System Trace's interval table.

Export the table first (docs/WORKFLOWS.md, Metal System Trace):

    xcrun xctrace export --input a.trace --output a-intervals.xml \
      --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'

then `python3 tools/summarize_gpu_intervals.py a-intervals.xml [b-intervals.xml]`. Each pass is
an encoder label over the vertex, fragment and compute channels, per frame (the passes run once a
frame: `--frames-by`). The GPU runs passes side by side, so an interval lasts as long as whatever
shares the GPU with it: a pass of a few rows beside the opaque pass spans the opaque pass. Each
moment's time is split evenly among the intervals running then, and the GPU's busy time is the
time anything runs, which is what a frame costs. The times follow the GPU's clock; compare two
traces by busy time and each pass's share of a reference pass in the same trace (`--reference`).
macOS lowers the GPU's performance state under sustained load, which slows every pass alike:
`--states` takes each trace's exported gpu-performance-state-intervals table and prints the share
of its time in each state.

A shader that needs more registers than the GPU has spills them to memory, and its pass runs few
threads at once: `--spills EVENTS ENCODERS` takes a recording's graphics-compiler-spill-events and
metal-application-encoders-list tables (recorded with `--instrument 'Metal GPU Counters'`) and
prints the bytes each pass spills.
"""

import argparse
import collections
import re
import xml.etree.ElementTree as ET

PROCESS = "yarra-app-game"


def rows(path):
    """An exported table's rows, each column as (formatted, raw) text."""
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
            yield row


def intervals(path):
    """(start ns, end ns, pass label, channel) of the game's intervals."""
    for row in rows(path):
        if PROCESS not in row.get("process", ("", ""))[0]:
            continue
        # "encoder:command buffer ( process )"; work without an encoder label (MetalFX's) shows
        # its command buffer's name first and its own label second.
        parts = row.get("event-label", ("", ""))[0].split("(")[0].split(":")
        label = parts[0].strip()
        if re.fullmatch(r"Command Buffer \d+", label) and len(parts) > 1 and parts[1].strip():
            label = parts[1].strip()
        label = re.sub(r"Command Buffer \d+", "Command Buffer", label)
        start = int(row["start"][1])
        end = start + int(row["duration"][1] or 0)
        yield start, end, label, row.get("channel-name", ("", ""))[0]


def performance_states(path):
    """Each GPU performance state's share of a trace's time, largest first."""
    time = collections.Counter()
    for row in rows(path):
        time[row.get("gpu-performance-state", ("?", ""))[0]] += int(row["duration"][1] or 0)
    total = max(sum(time.values()), 1)
    return [(state, ns / total) for state, ns in time.most_common()]


def spills(events, encoders):
    """(pass label, spilled bytes as formatted, recordings) of each pass that spills."""
    label = {row["encoder-id"][1]: row["encoder-label"][0] for row in rows(encoders)}
    count = collections.Counter()
    for row in rows(events):
        count[(label.get(row["encoder-id"][1], "?"), row["spilled-bytes"][0])] += 1
    return [(name, size, n) for (name, size), n in sorted(count.items())]


def summarize(path, frames_by, background=()):
    """Each pass's even share of GPU time and the busy time, ms per frame, and the frames.

    A background pass (`background`, labels) works in the gaps of the others, as the cloud
    volume's sweep does: it takes no share while another pass runs, only the time it runs alone,
    which is about the time it adds.
    """
    spans = list(intervals(path))
    events = sorted(
        [(start, 1, i) for i, (start, _, _, _) in enumerate(spans)]
        + [(end, -1, i) for i, (_, end, _, _) in enumerate(spans)]
    )
    shares = collections.Counter()
    busy = 0
    running = set()
    last = None

    for time, change, i in events:
        if running and time > last:
            busy += time - last
            sharing = [j for j in running if spans[j][2] not in background] or running
            for j in sharing:
                shares[spans[j][2]] += (time - last) / len(sharing)
        if change > 0:
            running.add(i)
        else:
            running.discard(i)
        last = time

    frames = max(sum(1 for s in spans if s[2] == frames_by and s[3] == "Fragment"), 1)
    passes = {label: ns / frames / 1e6 for label, ns in shares.items()}
    return passes, busy / frames / 1e6, frames


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("traces", nargs="+")
    parser.add_argument("--frames-by", default="temporal tone map to output")
    parser.add_argument("--reference", default="main_opaque_pass_3d")
    parser.add_argument("--top", type=int, default=30)
    parser.add_argument("--states", nargs="*", default=[])
    parser.add_argument("--spills", nargs=2, metavar=("EVENTS", "ENCODERS"))
    parser.add_argument("--background", nargs="*", default=[], help="passes that fill gaps")
    args = parser.parse_args()

    if args.spills:
        for name, size, n in spills(*args.spills):
            print(f"{name[:48]:48} spills {size} ({n} recorded)")

    tables = [summarize(path, args.frames_by, set(args.background)) for path in args.traces]

    def cell(passes, ms):
        share = ms / passes.get(args.reference, float("nan"))
        return f"{ms:7.2f} ms {share:6.2f}x"

    for path, (_, _, frames) in zip(args.traces, tables):
        print(f"{path}: {frames} frames")
    for path in args.states:
        shares = "  ".join(f"{state} {share:.0%}" for state, share in performance_states(path))
        print(f"{path}: GPU state {shares}")
    print(f"{'GPU busy':48} " + "  ".join(cell(p, busy) for p, busy, _ in tables))
    order = sorted(tables[0][0], key=lambda label: -tables[0][0][label])
    for label in order[: args.top]:
        cells = [cell(passes, passes.get(label, 0.0)) for passes, _, _ in tables]
        print(f"{label[:48]:48} " + "  ".join(cells))


if __name__ == "__main__":
    main()
