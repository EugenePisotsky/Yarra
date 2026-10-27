"""Heat, clocks and power over minutes of play, from a powermetrics log.

Record a log while playing, ten minutes from a cool Mac (docs/WORKFLOWS.md, Long play and heat):

    sudo powermetrics --samplers cpu_power,gpu_power,thermal -i 5000 -n 120 -o tmp/power/yarra.txt

then `python3 tools/power_report.py tmp/power/yarra.txt:56-576 [tmp/power/other.txt:16-595]`.
The range after a log is its play in seconds from the first sample, leaving out loading and the
moments after play. Each log is summarised: when macOS first raised its thermal pressure, how much
of the play ran under Heavy pressure, the GPU's clock, busy share and power while the Mac was cool
(Nominal) and while it was hot (Heavy or worse), and the clock over the last five minutes of play,
which is what long play runs at. That clock's share of the GPU's top clock is the factor between
the bench, which measures at the top clock, and long play.

`--samples` prints every sample of the play. `--record` appends each log's summary to
`tools/power_history.csv` under `--game` (default: Yarra and the checked-out commit) and `--note`.
"""

import argparse
import csv
import datetime
import os
import re
import subprocess

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HISTORY = os.path.join(ROOT, "tools/power_history.csv")
PRESSURES = ["Nominal", "Moderate", "Heavy", "Trapping", "Sleeping"]
SUSTAINED_SECONDS = 300


def samples(path):
    """Each sample as a dict: seconds from the first, thermal pressure, GPU clock and top clock
    (MHz), GPU busy share (%), GPU and CPU power (W) and its timestamp."""
    with open(path) as file:
        text = file.read()

    result = []
    for block in text.split("*** Sampled system activity")[1:]:
        stamp = datetime.datetime.strptime(re.search(r"\((.*?)\)", block).group(1),
                                           "%a %b %d %H:%M:%S %Y %z")

        def number(pattern):
            return float(re.search(pattern, block, re.M).group(1))

        residency = re.search(r"^GPU HW active residency:.*?\((.*)\)", block, re.M).group(1)
        pressure = re.search(r"^Current pressure level: (\w+)", block, re.M)
        result.append({
            "stamp": stamp,
            "pressure": pressure.group(1) if pressure else "Nominal",
            "mhz": number(r"^GPU HW active frequency: (\d+)"),
            "top_mhz": max(float(m) for m in re.findall(r"(\d+) MHz", residency)),
            "busy": number(r"^GPU HW active residency:\s+([\d.]+)"),
            "gpu_w": number(r"^GPU Power: (\d+)") / 1000,
            "cpu_w": number(r"^CPU Power: (\d+)") / 1000,
        })

    for sample in result:
        sample["t"] = (sample["stamp"] - result[0]["stamp"]).total_seconds()
    return result


def play(spec):
    """A log's samples within its play range (`path:START-END`, seconds)."""
    path, _, span = spec.partition(":")
    all_samples = samples(path)
    start, end = (float(s) for s in span.split("-")) if span else (0, all_samples[-1]["t"])
    return path, start, end, [s for s in all_samples if start <= s["t"] <= end]


def summary(start, end, played):
    """The figures one log is judged by, as text for the table and the history."""

    def mean(key, rows):
        return sum(r[key] for r in rows) / len(rows) if rows else float("nan")

    def clock(rows):
        return f"{mean('mhz', rows):.0f} MHz, {mean('busy', rows):.1f}% busy" if rows else "-"

    cool = [s for s in played if s["pressure"] == "Nominal"]
    hot = [s for s in played if PRESSURES.index(s["pressure"]) >= PRESSURES.index("Heavy")]
    raised = next((s for s in played if s["pressure"] != "Nominal"), None)
    sustained = [s for s in played if s["t"] >= end - SUSTAINED_SECONDS]
    top = max(s["top_mhz"] for s in played)
    logged = played[0]["stamp"] - datetime.timedelta(seconds=played[0]["t"])

    return {
        "date": logged.strftime("%Y-%m-%dT%H:%M"),
        "play_s": f"{start:.0f}-{end:.0f}",
        "first_pressure_s": f"{raised['t']:.0f}" if raised else "",
        "heavy_pct": f"{100 * len(hot) / len(played):.0f}",
        "cool": clock(cool),
        "hot": clock(hot),
        "sustained_mhz": f"{mean('mhz', sustained):.0f}",
        "sustained_of_top": f"{mean('mhz', sustained) / top:.2f}",
        "play_w": f"{mean('gpu_w', played):.1f} + {mean('cpu_w', played):.1f}",
        "cool_w": f"{mean('gpu_w', cool):.1f} + {mean('cpu_w', cool):.1f}" if cool else "-",
        "hot_w": f"{mean('gpu_w', hot):.1f} + {mean('cpu_w', hot):.1f}" if hot else "-",
    }


ROWS = [
    ("play (s)", "play_s"),
    ("pressure first raised (s)", "first_pressure_s"),
    ("play under Heavy (%)", "heavy_pct"),
    ("GPU while cool", "cool"),
    ("GPU while hot", "hot"),
    ("GPU + CPU W over play", "play_w"),
    ("GPU + CPU W cool", "cool_w"),
    ("GPU + CPU W hot", "hot_w"),
    ("last 5 min clock (MHz)", "sustained_mhz"),
    ("... of the top clock", "sustained_of_top"),
]


def print_samples(path, played):
    print(f"== {path}")
    print("    t  pressure   GPU MHz  busy%  GPU W  CPU W")
    for s in played:
        print(f"{s['t']:5.0f}  {s['pressure']:9s} {s['mhz']:7.0f} {s['busy']:6.1f} "
              f"{s['gpu_w']:6.1f} {s['cpu_w']:6.1f}")


def print_table(names, summaries):
    width = max(26, *(len(n) + 2 for n in names))
    print(" " * 26 + "".join(f"{n:>{width}}" for n in names))
    for label, key in ROWS:
        cells = [s[key] or "-" for s in summaries]
        print(f"{label:26s}" + "".join(f"{c:>{width}}" for c in cells))


def commit():
    return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()


def append_history(game, note, summaries):
    fields = ["date", "game", "note"] + [key for _, key in ROWS]
    new = not os.path.exists(HISTORY)
    with open(HISTORY, "a", newline="") as file:
        writer = csv.DictWriter(file, fields, lineterminator="\n")
        if new:
            writer.writeheader()
        for s in summaries:
            writer.writerow({"game": game, "note": note, **s})
    print(f"appended {len(summaries)} rows to {os.path.relpath(HISTORY, ROOT)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("logs", nargs="+", help="powermetrics logs, each as PATH[:START-END]")
    parser.add_argument("--samples", action="store_true", help="print every sample of the play")
    parser.add_argument("--record", action="store_true", help="append to the history")
    parser.add_argument("--game", default=f"Yarra {commit()}", help="what ran, for the history")
    parser.add_argument("--note", default="", help="the scene and setup, for the history")
    args = parser.parse_args()

    names, summaries = [], []
    for spec in args.logs:
        path, start, end, played = play(spec)
        if args.samples:
            print_samples(path, played)
        names.append(os.path.basename(path))
        summaries.append(summary(start, end, played))

    print_table(names, summaries)
    if args.record:
        append_history(args.game, args.note, summaries)


if __name__ == "__main__":
    main()
