"""The frame's GPU cost in fixed views, against the frame budget.

    cargo build --release -p yarra-app-game
    python3 tools/bench.py [--views meadow,forest-pine] [--note TEXT] [-- extra game flags]

Each view in `tools/frame_budget.toml` runs the play setup uncapped from a held clock: fullscreen,
50% + MetalFX Temporal, 10:00, the view's weather. Once the timed profile starts measuring, a 3 s
Metal System Trace gives the GPU's busy time per frame and each pass's share of it
(`summarize_gpu_intervals.py`); the budget's lines group the passes. Uncapped, the GPU stays at
its Maximum performance state; a view that left it is flagged and kept out of the history, since
every pass then slows alike. The views run with a pause between them for the same reason. The
trace's state table covers only its first half second and often holds just momentary Minimum
transitions, so without a lasting state the clock is read from MetalFX Temporal's own pass, the
same work every frame.

Rows append to `tools/bench_history.csv` with the commit and note, so a change's cost is the
difference between two rows of one view. Flags after `--` go to every run (an A/B toggle such as
`--profile-grass off`); say so in the note.
"""

import argparse
import collections
import csv
import datetime
import glob
import os
import re
import subprocess
import sys
import time
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize_gpu_intervals import performance_states, summarize  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GAME = os.path.join(ROOT, "target/release/yarra-app-game")
BUDGET = os.path.join(ROOT, "tools/frame_budget.toml")
HISTORY = os.path.join(ROOT, "tools/bench_history.csv")
WORK = os.path.join(ROOT, "tmp/bench")
TABLE = "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"{}\"]"

# MetalFX Temporal's own pass at the play setup and the GPU's Maximum state (1.06–1.09 ms in
# every view); a slower one means a lower clock.
CLOCK_PASS = "MetalFX Temporal"
CLOCK_MAXIMUM_MS = 1.15

Result = collections.namedtuple(
    "Result", "view busy frames state fps main_ms prep_ms lines"
)


def wait_for(path, text, process, seconds):
    """Whether `text` appeared in the log before the game exited or the time ran out."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and process.poll() is None:
        with open(path, errors="replace") as log:
            if text in log.read():
                return True
        time.sleep(0.5)
    return False


def record(view, extra, budget):
    """Run one view and trace it; the exported tables and the game's log, or None."""
    name = view["name"]
    log_path = os.path.join(WORK, f"{name}.log")
    trace = os.path.join(WORK, f"{name}.trace")
    tables = {
        schema: os.path.join(WORK, f"{name}-{schema}.xml")
        for schema in ("metal-gpu-intervals", "gpu-performance-state-intervals")
    }
    subprocess.run(["rm", "-rf", trace, *tables.values()], check=True)

    command = [
        GAME,
        "--start-view", os.path.join(ROOT, view["file"]),
        "--profile-seconds", str(budget["profile_seconds"]),
        "--profile-warmup", str(budget["warmup_seconds"]),
        "--profile-fps", "0",
        "--upscaler", "metalfx-temporal",
        "--day-clock", "off",
        "--time", budget["time"],
        "--weather", view.get("weather", "clear"),
        "--timing-log",
        *view.get("args", []),
        *extra,
    ]
    environment = dict(os.environ, RUST_LOG="info,wgpu=error,naga=warn")
    with open(log_path, "w") as log:
        game = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                env=environment)

    if not wait_for(log_path, "event=measure_start", game, budget["warmup_seconds"] + 90):
        game.kill()
        print(f"{name}: the profile never started measuring; see {log_path}")
        return None

    # Recording starts a few seconds after the call; the profile outlasts it.
    subprocess.run(["xcrun", "xctrace", "record", "--template", "Metal System Trace",
                    "--attach", "yarra-app-game", "--time-limit", "3s", "--output", trace],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for schema, path in tables.items():
        subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--output", path,
                        "--xpath", TABLE.format(schema)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    try:
        game.wait(timeout=budget["profile_seconds"] + 60)
    except subprocess.TimeoutExpired:
        game.kill()
    # Each recording leaves about 1 GB of kernel trace behind.
    subprocess.run(["rm", "-rf", trace], check=True)
    for leftover in glob.glob(os.path.join(os.environ.get("TMPDIR", "/tmp"),
                                           "instruments*.ktrace")):
        os.remove(leftover)

    if not all(os.path.exists(path) for path in tables.values()):
        print(f"{name}: the trace was not exported")
        return None
    return tables, log_path


def measured_log(path):
    """Mean update fps, CPU main work and render preparation (ms) once measuring began."""
    fps, main, prep = [], [], []
    measuring = False
    with open(path, errors="replace") as log:
        for line in log:
            measuring = measuring or "event=measure_start" in line
            if not measuring:
                continue
            if m := re.search(r"event=sample .*update_fps=([0-9.]+)", line):
                fps.append(float(m.group(1)))
            if m := re.search(r"main_work_ms=Some\(([0-9.]+)\)", line):
                main.append(float(m.group(1)))
            if m := re.search(r"render_prep_ms=Some\(([0-9.]+)\)", line):
                prep.append(float(m.group(1)))

    def mean(values):
        return sum(values) / len(values) if values else float("nan")

    return mean(fps), mean(main), mean(prep)


def group(passes, lines):
    """Each budget line's ms per frame, and the passes no line claims."""
    totals = collections.Counter()
    unclaimed = {}
    for label, ms in passes.items():
        line = next((line["name"] for line in lines
                     if any(re.search(p, label) for p in line["passes"])), None)
        if line is None:
            unclaimed[label] = ms
        else:
            totals[line] += ms
    return totals, unclaimed


def commit():
    """The checked-out commit, marked when tracked files the game reads have changes (not the
    docs or this history)."""
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no", "--", ".",
                            ":!docs", ":!tools", ":!AGENTS.md"], cwd=ROOT,
                           capture_output=True, text=True).stdout.strip()
    return head + ("+" if dirty else "")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--views", help="comma-separated view names (default: all)")
    parser.add_argument("--note", default="", help="what this run measures, for the history")
    parser.add_argument("--no-history", action="store_true", help="print without appending")
    parser.add_argument("extra", nargs="*", help="flags passed to every run, after --")
    args = parser.parse_args()

    with open(BUDGET, "rb") as file:
        budget = tomllib.load(file)
    lines = budget["line"]
    views = budget["view"]
    if args.views:
        wanted = args.views.split(",")
        views = [view for view in views if view["name"] in wanted]
        missing = set(wanted) - {view["name"] for view in views}
        if missing:
            sys.exit(f"unknown views: {', '.join(sorted(missing))}")
    if not os.path.exists(GAME):
        sys.exit("build first: cargo build --release -p yarra-app-game")
    os.makedirs(WORK, exist_ok=True)

    revision = commit()
    stamp = datetime.datetime.now().isoformat(timespec="minutes")
    results = []
    for index, view in enumerate(views):
        # Minutes of full load drop the GPU to its Medium state; idling lets it recover.
        if index > 0:
            time.sleep(budget["cooldown_seconds"])
        result = measure(view, args.extra, budget)
        if result is not None:
            results.append(result)

    print_table(results, lines, budget["total_ms"])
    at_maximum = [r for r in results if r.state == "Maximum"]
    if not args.no_history:
        append_history(at_maximum, lines, revision, stamp, args.note, args.extra)


def measure(view, extra, budget):
    """One view's costs per budget line, or None when it could not be traced."""
    recorded = record(view, extra, budget)
    if recorded is None:
        return None
    tables, log_path = recorded
    passes, busy, frames = summarize(tables["metal-gpu-intervals"], budget["frames_by"],
                                     set(budget["background"]))
    state = gpu_state(tables["gpu-performance-state-intervals"], passes)
    fps, main_ms, prep_ms = measured_log(log_path)
    totals, unclaimed = group(passes, budget["line"])
    for label, ms in sorted(unclaimed.items(), key=lambda item: -item[1]):
        if ms >= 0.05:
            print(f"{view['name']}: no budget line for {label!r} ({ms:.2f} ms)")
    return Result(view["name"], busy, frames, state, fps, main_ms, prep_ms, totals)


def gpu_state(states_path, passes):
    """The GPU's performance state through the trace: the table's longest lasting state, or,
    when it holds only momentary Minimum transitions, judged by the clock pass."""
    lasting = [(state, share) for state, share in performance_states(states_path)
               if state != "Minimum"]
    if lasting:
        return max(lasting, key=lambda item: item[1])[0]
    return "Maximum" if passes.get(CLOCK_PASS, float("inf")) <= CLOCK_MAXIMUM_MS else "lower"


def print_table(results, lines, total):
    """Budget lines down, views across; '!' marks a line over its budget."""
    if not results:
        return
    width = max(len(line["name"]) for line in lines) + 2

    def row(name, limit, values):
        cells = "".join(
            f"{value:8.2f}{'!' if limit is not None and value > limit else ' '}"
            for value in values
        )
        limit_cell = f"{limit:6.2f}" if limit is not None else "      "
        print(f"{name:{width}}{limit_cell}  {cells}")

    print(f"{'':{width}}budget  " + "".join(f"{r.view[:8]:>9}" for r in results))
    row("GPU busy", total, [r.busy for r in results])
    for line in lines:
        row(line["name"], line["ms"], [r.lines.get(line["name"], 0.0) for r in results])
    row("other", None, [r.busy - sum(r.lines.values()) for r in results])
    row("fps", None, [r.fps for r in results])
    row("CPU main", None, [r.main_ms for r in results])
    row("render prep", None, [r.prep_ms for r in results])
    print(f"{'GPU state':{width}}        " + "".join(f"{r.state[:8]:>9}" for r in results))
    for r in results:
        if r.state != "Maximum":
            print(f"{r.view}: the GPU left its Maximum state, so it stays out of the history")


def append_history(results, lines, revision, stamp, note, extra):
    """One row per view: when, which commit, what was measured, and the costs."""
    fields = ["date", "commit", "note", "flags", "view", "frames", "gpu_state", "fps",
              "cpu_main_ms", "render_prep_ms", "gpu_busy_ms"] + [line["name"] for line in lines]
    new = not os.path.exists(HISTORY)
    with open(HISTORY, "a", newline="") as file:
        writer = csv.writer(file, lineterminator="\n")
        if new:
            writer.writerow(fields)
        for r in results:
            writer.writerow([
                stamp, revision, note, " ".join(extra), r.view, r.frames, r.state,
                f"{r.fps:.1f}", f"{r.main_ms:.2f}", f"{r.prep_ms:.2f}", f"{r.busy:.2f}",
            ] + [f"{r.lines.get(line['name'], 0.0):.2f}" for line in lines])
    print(f"appended {len(results)} rows to {os.path.relpath(HISTORY, ROOT)}")


if __name__ == "__main__":
    main()
