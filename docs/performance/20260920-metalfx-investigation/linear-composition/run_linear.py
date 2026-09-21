import datetime as dt
import json
import os
from pathlib import Path
import re
import signal
import statistics as stats
import subprocess
import sys

root = Path(__file__).resolve().parent
case, bundle, *args = sys.argv[1:]

def stop(signum, frame):
    raise SystemExit(128 + signum)

signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
game = power = None
try:
    # Do not pause or otherwise alter another running game during diagnostics.
    for cmd in subprocess.check_output(['ps', '-axo', 'comm='], text=True).splitlines():
        if Path(cmd.strip()).name in {'yarra-app-game', 'YarraReleaseProfile'}:
            raise SystemExit('Close the existing release game before running this benchmark.')
    env = dict(os.environ, MTL_HUD_ENABLED='1', MTL_HUD_LOG_ENABLED='1', RUST_LOG='warn', NO_COLOR='1')
    with (root / (case + '.log')).open('w') as log, (root / (case + '.power.jsonl')).open('w') as telemetry:
        power = subprocess.Popen(['tmp/terrain-gpu-headroom/macmon', 'pipe', '--samples', '30'], stdout=telemetry, stderr=subprocess.DEVNULL)
        command = [str(root / bundle / 'Contents/MacOS/empty_frame'), *args]
        game = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            code = game.wait(timeout=25)
        except subprocess.TimeoutExpired:
            game.terminate()
            code = game.wait(timeout=3)
finally:
    if game is not None and game.poll() is None:
        game.kill()
        game.wait()
    if power is not None and power.poll() is None:
        power.terminate()
        power.wait(timeout=3)

lines = (root / (case + '.log')).read_text().splitlines()
start = next((i for i, s in enumerate(lines) if 'event=measure_start' in s), None)
end = next((i for i, s in enumerate(lines) if 'event=complete' in s), None)
result = dict(case=case, code=code, command=command, errors=[s for s in lines if 'ERROR' in s or 'panicked' in s])
if start is not None and end is not None:
    packets = []
    for s in lines[start + 1:end]:
        if 'metal-HUD: ' not in s: continue
        payload = s.split('metal-HUD: ')[1].strip()
        if not packets or packets[-1] != payload: packets.append(payload)
    packets = packets[1:]
    gpu = [float(v) for s in packets for v in s.split(',')[4::2]]
    interval = [float(v) for s in packets for v in s.split(',')[3::2]]
    unix = lambda s: int(re.search(r'unix_ms=(\d+)', s)[1]) / 1000
    samples = [json.loads(s) for s in (root / (case + '.power.jsonl')).read_text().splitlines()]
    samples = [s for s in samples if unix(lines[start]) + 1 <= dt.datetime.fromisoformat(s['timestamp']).timestamp() <= unix(lines[end])]
    result.update(valid_focus=('focused=true' in lines[start] and 'focused=true' in lines[end]), start=lines[start], complete=lines[end], packets=len(packets), hud_gpu_median=stats.median(gpu) if gpu else None, hud_interval_mean=stats.mean(interval) if interval else None, power_samples=len(samples))
    for field in ['gpu_power', 'cpu_power', 'gpu_freq_mhz', 'gpu_active_ratio']:
        if samples: result[field + '_mean'] = stats.mean(s[field] for s in samples)
(root / (case + '.json')).write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result), flush=True)
