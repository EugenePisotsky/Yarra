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
case = 'native-empty'

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
        command = [str(root / 'native-empty')]
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

events = [json.loads(s) for s in (root / (case + '.log')).read_text().splitlines() if s.startswith('{')]
start = next(s for s in events if s['event'] == 'measure_start')
end = next(s for s in events if s['event'] == 'complete')
samples = [json.loads(s) for s in (root / (case + '.power.jsonl')).read_text().splitlines()]
samples = [s for s in samples if start['unix_ms']/1000 + 1 <= dt.datetime.fromisoformat(s['timestamp']).timestamp() <= end['unix_ms']/1000]
result = dict(case=case, code=code, command=command, events=events, power_samples=len(samples))
for field in ['gpu_power', 'cpu_power', 'gpu_freq_mhz', 'gpu_active_ratio']:
    if samples: result[field + '_mean'] = stats.mean(s[field] for s in samples)
(root / (case + '.json')).write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result), flush=True)
