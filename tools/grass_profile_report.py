"""Parse profiling artifacts. No external dependencies; unavailable metrics stay null."""
import datetime as dt
import html
import json
from pathlib import Path
import re
import statistics


def fields(line):
    return dict(re.findall(r'(\w+)=(\[[^]]*\]|[^\s]+)', line))


def parse_power(text):
    headers = list(re.finditer(
        r'^\*\*\* Sampled system activity \((.*?)\) \(([\d.]+)ms elapsed\) \*\*\*$', text, re.M))
    result = []
    patterns = {
        'cpu_w': (r'^CPU Power:\s*([\d.]+) mW', .001),
        # The processor section is first; do not double-count the later GPU section.
        'gpu_w': (r'^GPU Power:\s*([\d.]+) mW', .001),
        'gpu_mhz': (r'^GPU HW active frequency:\s*([\d.]+) MHz', 1),
        'gpu_active_percent': (r'^GPU HW active residency:\s*([\d.]+)%', 1),
    }
    for i, header in enumerate(headers):
        block = text[header.end():headers[i + 1].start() if i + 1 < len(headers) else len(text)]
        try:
            end = dt.datetime.strptime(header[1], '%a %b %d %H:%M:%S %Y %z').timestamp()
        except ValueError:
            continue
        duration = float(header[2]) / 1000
        if duration <= 0:
            continue
        row = {'start_s': end - duration, 'end_s': end, 'duration_s': duration}
        for key, (pattern, factor) in patterns.items():
            match = re.search(pattern, block, re.M)
            row[key] = float(match[1]) * factor if match else None
        pressure = re.search(r'^Current pressure level:\s*(\S+)', block, re.M)
        row['thermal'] = pressure[1].lower() if pressure else None
        result.append(row)
    return result


def power_window(rows, start, end):
    # Whole samples only: no pretending to know which part of a boundary sample was idle/warmup.
    selected = [r for r in rows if r['start_s'] >= start and r['end_s'] <= end]
    result = {'samples': len(selected), 'coverage_s': sum(r['duration_s'] for r in selected)}
    for key in ('cpu_w', 'gpu_w', 'gpu_mhz', 'gpu_active_percent'):
        valid = [r for r in selected if r[key] is not None]
        weight = sum(r['duration_s'] for r in valid)
        result[key] = sum(r[key] * r['duration_s'] for r in valid) / weight if weight else None
        result[key + '_coverage_s'] = weight
    paired = [r for r in selected if r['cpu_w'] is not None and r['gpu_w'] is not None]
    paired_duration = sum(r['duration_s'] for r in paired)
    result['cpu_gpu_w'] = (sum((r['cpu_w'] + r['gpu_w']) * r['duration_s'] for r in paired) / paired_duration
                           if paired_duration else None)
    result['thermal_states'] = sorted({r['thermal'] for r in selected if r['thermal']})
    return result


def timestamp(line, utc_offset=None):
    # Metal HUD timestamps use local wall time, unlike Bevy's UTC log prefix.
    match = re.match(r'(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+)', line)
    if not match:
        return None
    value = dt.datetime.fromisoformat(match[1])
    if utc_offset is not None:
        value = value.replace(tzinfo=dt.timezone(dt.timedelta(seconds=utc_offset)))
    return value.astimezone().timestamp()


def quantile(values, q):
    return sorted(values)[int((len(values) - 1) * q)] if values else None


def analyze_run(directory, power_rows):
    directory = Path(directory)
    meta = json.loads((directory / 'run.json').read_text())
    text = (directory / 'game.log').read_text(errors='replace') if (directory / 'game.log').exists() else ''
    events = [fields(l) for l in text.splitlines() if 'GRASS_PROFILE ' in l]
    audits = [fields(l) for l in text.splitlines() if 'RENDER_AUDIT ' in l]
    start = next((e for e in events if e.get('event') == 'measure_start'), None)
    end = next((e for e in events if e.get('event') == 'complete'), None)
    result = {'name': meta['name'], 'settings': meta['settings'], 'inputs': meta['inputs'],
              'errors': [], 'target_misses': [], 'warnings': [], 'power': {}, 'hud': {}, 'updates': {}}
    if meta.get('exit_code') != 0:
        result['errors'].append(f"Game exit status: {meta.get('exit_code', 'incomplete')}")
    if 'panicked at' in text or re.search(r'\bERROR\b', text):
        result['errors'].append('Game log contains a rendering/runtime error')
    if not start or not end:
        result['errors'].append('Missing measurement start/completion; partial run')
        return result
    begin, finish = int(start['unix_ms']) / 1000, int(end['unix_ms']) / 1000
    result.update(start_s=begin, end_s=finish, duration_s=float(end['measured_s']))
    samples = [e for e in events if e.get('event') == 'sample']
    frames = sum(int(e['frames']) for e in samples)
    duration = sum(float(e['window_s']) for e in samples)
    result['updates'] = {
        'frames': frames, 'fps': frames / duration if duration else None,
        'late_updates': sum(int(e['late_updates']) for e in samples),
        'max_interval_ms': max((float(e['update_max_ms']) for e in samples), default=None),
        'worst_window_p95_ms': max((float(e['update_p95_ms']) for e in samples), default=None),
    }
    if any(e.get('focused') != 'true' for e in [start, *samples]):
        result['errors'].append('Focus lost during measurement')
    settings = meta['settings']
    # The final partial sample can be arbitrarily short. Inspect full sampling
    # windows as well as the mean so brief stalls cannot disappear in a long run.
    cadence = [(e, int(e['frames']) / float(e['window_s']))
               for e in samples if float(e['window_s']) >= .9]
    worst = min(cadence, key=lambda pair: pair[1], default=None)
    below = [e for e, rate in cadence if settings['fps'] and rate < .95 * settings['fps']]
    result['updates'].update(
        worst_window_fps=worst[1] if worst else None,
        worst_window_duration_s=float(worst[0]['window_s']) if worst else None,
        worst_window_end_s=(int(worst[0]['unix_ms']) / 1000 - begin) if worst else None,
        below_95pct_target_windows=len(below),
        below_95pct_target_window_s=sum(float(e['window_s']) for e in below),
        late_update_fraction=result['updates']['late_updates'] / frames if frames else None,
    )
    if below:
        result['warnings'].append(
            f"Brief/periodic cadence loss: {len(below)} full app sampling window(s) below 95% of target; "
            "inspect the timeline even if average FPS passes")
    fps = result['updates']['fps']
    if settings['fps'] and (fps is None or abs(fps / settings['fps'] - 1) > .03):
        result['target_misses'].append('Application update rate differs from requested target by >3%')
    selected_audits = [a for a in audits if begin <= int(a['unix_ms']) / 1000 <= finish]
    if not selected_audits:
        result['errors'].append('No in-window settings/residency audit')
    expected = {'msaa_samples': str(settings['msaa']),
                'density': {'balanced': 'Balanced', 'full': 'FullReference', 'authored': 'Authored'}[settings['density']],
                'grass': 'full' if settings['grass'] == 'full' else 'disabled',
                'counters': str(settings['counters']).lower(), 'prepass': 'false'}
    if settings['size'] == 'game':
        for audit in selected_audits:
            surface = audit.get('surface_px', '')
            scale = audit.get('scale', '')
            if not re.fullmatch(r'\d+x\d+', surface) or scale != '0.75':
                result['errors'].append('Normal game scale or surface dimensions unavailable/unexpected')
                break
            target = 'x'.join(str(max(1, int(int(n) * .75))) for n in surface.split('x'))
            if audit.get('render_px') != target:
                result['errors'].append(f'Actual render_px does not match normal game scale: expected {target}')
                break
    else:
        expected['render_px'] = settings['size']
    # Old reports have no requested window mode or mode audit. Preserve their interpretation.
    if 'window' in settings:
        expected['window_mode'] = settings['window']
    if settings.get('prepared_blades') is not None:
        # Instance lookup table + requested physical blade records + indirect dispatch.
        expected['blade_preparation_bytes'] = str(851968 * 4 + settings['prepared_blades'] * 128 + 12)
    for key, value in expected.items():
        if any(a.get(key) != value for a in selected_audits):
            result['errors'].append(f'Actual {key} does not match requested {value}')
    # Also flag interactive changes beyond the requested controls.
    for key in ('shadows', 'ground_shader', 'terrain_prepared', 'wind', 'ui', 'lighting', 'render_path', 'render_px', 'surface_px', 'window_mode', 'monitor_px', 'monitor_hz', 'scale_factor', 'low_power'):
        if len({a.get(key) for a in selected_audits}) > 1:
            result['errors'].append(f'{key} changed during measurement')
    if any(a.get('terrain_prepared_active') != a.get('terrain_prepared_pages') for a in selected_audits):
        result['warnings'].append('Prepared terrain was not fully ready in every audit sample')
    if settings['view'] != 'grass-stream' and len({a.get('source_revision') for a in selected_audits}) > 1:
        result['warnings'].append('Source residency changed during measurement; check warmup duration')
    if any(any(json.loads(a.get('sampled_capacity_drops', '[0,0,0,0]'))) for a in selected_audits):
        result['errors'].append('Grass instance capacity drops')
    result['last_audit'] = selected_audits[-1] if selected_audits else None
    last = result['last_audit'] or {}
    result['display'] = {key: last.get(key) for key in ('window_mode', 'render_px', 'surface_px', 'monitor_px', 'monitor_hz', 'window_logical', 'scale_factor')}
    result['thermal_states'] = sorted({a.get('thermal', 'unknown') for a in selected_audits})
    hud_gpu, hud_interval = [], []
    previous_payload = None
    previous_time = None
    for line in text.splitlines():
        if 'metal-HUD: ' not in line:
            continue
        stamp = timestamp(line, meta.get('local_utc_offset_seconds'))
        payload = line.split('metal-HUD: ', 1)[1].strip()
        if payload == previous_payload:
            continue
        previous_payload = payload
        if stamp is None:
            continue
        # A HUD packet summarizes preceding work. Discard the first packet after the boundary.
        inside = previous_time is not None and previous_time >= begin and stamp <= finish
        previous_time = stamp
        if not inside:
            continue
        try:
            values = list(map(float, payload.split(',')))
        except ValueError:
            result['warnings'].append('Unrecognized Metal HUD packet')
            continue
        if len(values) < 5 or (len(values) - 3) % 2:
            result['warnings'].append('Unrecognized Metal HUD packet shape')
            continue
        hud_gpu.extend(values[4::2])
        hud_interval.extend(values[3::2])
    if hud_gpu:
        result['hud'] = {'samples': len(hud_gpu), 'gpu_mean_ms': statistics.mean(hud_gpu),
                         'gpu_p95_ms': quantile(hud_gpu, .95),
                         'presentation_interval_mean_ms': statistics.mean(hud_interval),
                         'presentation_interval_p95_ms': quantile(hud_interval, .95),
                         'scope': 'Correlated/overlapping HUD samples, not independent frame statistics'}
        if settings['fps'] and abs(statistics.mean(hud_interval) / (1000 / settings['fps']) - 1) > .05:
            result['target_misses'].append('HUD presentation rate differs from target by >5%')
        if settings['fps']:
            # Averages hide alternating short/long presentations (for example 8.3/25 ms
            # at an average 60 fps). This is a screening heuristic, not a dropped-frame count.
            threshold = 1.25 * 1000 / settings['fps']
            result['hud']['presentation_tail_sample_fraction'] = sum(v > threshold for v in hud_interval) / len(hud_interval)
            if result['hud']['presentation_interval_p95_ms'] > threshold:
                result['warnings'].append('HUD presentation interval p95 exceeds 1.25x target interval; inspect frame pacing (correlated samples)')
    else:
        result['warnings'].append('No Metal HUD samples; presentation rate is unverified')
    result['power'] = power_window(power_rows, begin, finish)
    watts = result['power']['cpu_gpu_w']
    result['power']['energy_per_app_frame_mj'] = watts * 1000 / fps if watts is not None and fps else None
    if result['power']['coverage_s'] < duration * .8:
        message = 'Power telemetry covers less than 80% of measurement window'
        result['errors' if meta.get('power_required') else 'warnings'].append(message)
    if meta.get('power_required') and any(result['power'][k + '_coverage_s'] < duration * .8 for k in ('cpu_w', 'gpu_w', 'gpu_mhz', 'gpu_active_percent')):
        result['errors'].append('Required power/frequency/activity telemetry unavailable')
    if settings['counters']:
        result['warnings'].append('Counter-enabled workload diagnostic; exclude from normal timing/energy comparisons')
    if any(s not in ('nominal', 'unknown') for s in result['thermal_states'] + result['power']['thermal_states']):
        result['warnings'].append('Elevated thermal pressure: inspect timeline and repeated-run order')
    result['samples'] = samples
    result['warnings'] = sorted(set(result['warnings']))
    return result


def number(value, places=2):
    return '—' if value is None else f'{value:.{places}f}'


def status(run):
    return ('INVALID' if run['errors'] else 'MISSED TARGET' if run['target_misses']
            else 'CHECK' if run['warnings'] else 'OK')


def write_report(root):
    root = Path(root)
    rows = parse_power((root / 'power.txt').read_text(errors='replace')) if (root / 'power.txt').exists() else []
    runs = [analyze_run(p.parent, rows) for p in sorted((root / 'runs').glob('*/run.json'))]
    manifest = json.loads((root / 'session.json').read_text())
    idle = manifest.get('idle_window')
    baseline = power_window(rows, *idle) if idle else None
    report = {'scope': 'Same-device comparison; subsystem power estimates, not grass-attributed watts',
              'idle': baseline, 'runs': runs, 'session': manifest, 'power_samples': rows}
    (root / 'report.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    lines = ['# Grass profiling report', '', f"Session: {manifest.get('status', 'unknown')}.",
             *([f"Error: {manifest['error']}"] if manifest.get('error') else []), '',
             'Power and clock context take priority over small HUD-duration differences. Invalid runs remain visible.', '',
             '| Run | Status | Window | World pixels | Surface pixels | App fps | Worst app window fps | GPU W | CPU W | GPU MHz | Active % | mJ/app frame | HUD GPU ms | HUD interval p95 ms |',
             '| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |']
    table_rows = []
    for run in runs:
        power = run['power']
        values = [run['updates'].get('fps'), run['updates'].get('worst_window_fps'), power.get('gpu_w'), power.get('cpu_w'), power.get('gpu_mhz'),
                  power.get('gpu_active_percent'), power.get('energy_per_app_frame_mj'), run['hud'].get('gpu_mean_ms'),
                  run['hud'].get('presentation_interval_p95_ms')]
        display = run.get('display', {})
        cells = [run['name'], status(run), *[display.get(k) or 'unavailable' for k in ('window_mode', 'render_px', 'surface_px')], *map(number, values)]
        lines.append('| ' + ' | '.join(cells) + ' |')
        table_rows.append('<tr>' + ''.join('<td>' + html.escape(cell) + '</td>' for cell in cells) + '</tr>')
    if baseline:
        lines += ['', f"App-closed idle estimate: GPU {number(baseline['gpu_w'])} W, CPU {number(baseline['cpu_w'])} W. Not automatically subtracted."]
    lines += ['', 'mJ/app frame uses system CPU+GPU estimates divided by application updates, not independently counted presentations. '
              'Worst app window uses sampling windows of at least 0.9 seconds (normally about one second), not a 1% low or presentation counter. '
              'HUD samples overlap. Power timestamps have approximately one-second alignment precision. '
              'The table does not declare an optimization winner or establish cross-device equivalence.']
    for run in runs:
        lines += ['', f"## {run['name']}", '', f"Settings: `{json.dumps(run['settings'], sort_keys=True)}`", '',
                  f"Observed display: `{json.dumps(run.get('display', {}), sort_keys=True)}`", '',
                  f"Application thermal states: {', '.join(run.get('thermal_states', [])) or 'unavailable'}. "
                  f"Power sampler: {', '.join(run['power'].get('thermal_states', [])) or 'unavailable'}."]
        updates = run['updates']
        if updates.get('worst_window_fps') is not None:
            lines += ['', f"Worst app window: {number(updates['worst_window_fps'])} fps over "
                      f"{number(updates['worst_window_duration_s'], 3)} s, ending "
                      f"{number(updates['worst_window_end_s'])} s into measurement. "
                      f"Windows below 95% of target: {updates['below_95pct_target_windows']} "
                      f"({number(updates['below_95pct_target_window_s'])} s of sampled time). "
                      f"Late updates: {updates['late_updates']} / {updates['frames']}."]
        lines += [f'- {message}' for message in run['errors'] + run['target_misses'] + run['warnings']]
    (root / 'report.md').write_text('\n'.join(lines) + '\n')
    # Portable timeline: no CDN, server, or installed plotting packages.
    data = json.dumps(report).replace('<', '\\u003c')
    page = '''<!doctype html><meta charset="utf-8"><title>Grass profile</title>
<style>body{font:15px system-ui;margin:32px;background:#101820;color:#e8f1f5}pre{white-space:pre-wrap;line-height:1.5}canvas{width:100%;height:230px;background:#18232d;margin:12px 0}select{font:inherit}h1{font-size:25px}table{border-collapse:collapse;width:100%;margin:20px 0}td,th{text-align:right;padding:10px;border-bottom:1px solid #354551;white-space:nowrap}td:first-child,th:first-child{text-align:left}.table{overflow-x:auto}summary{cursor:pointer;padding:16px 0}</style>
<h1>Grass profiling</h1><p>Whole-system power estimates. Inspect clocks, thermal state and frame delivery together.</p>
<p id="session">SESSION</p>
<div class="table"><table><thead><tr><th>Run</th><th>Status</th><th>Window</th><th>World pixels</th><th>Surface pixels</th><th>App fps</th><th>Worst app window fps</th><th>GPU W</th><th>CPU W</th><th>GPU MHz</th><th>Active %</th><th>mJ/app frame</th><th>HUD GPU ms</th><th>HUD interval p95 ms</th></tr></thead><tbody>ROWS</tbody></table></div>
<select id="metric" aria-label="Timeline metric"><option value="app_fps">Application cadence · updates/s (normally ~1 s windows)</option><option value="gpu_w">GPU power · W</option><option value="cpu_w">CPU power · W</option><option value="gpu_mhz">GPU active frequency · MHz</option><option value="gpu_active_percent">GPU active residency · %</option></select>
<p>Thermal shading: amber = moderate; orange = heavy. These are sampled pressure labels, not temperatures or fan speeds.</p>
<canvas id="plot"></canvas><details><summary>Run details and interpretation</summary><pre>SUMMARY</pre></details><script>const report=DATA;
const canvas=document.querySelector('#plot'), select=document.querySelector('#metric');
function draw(){
  const key=select.value;
  let rows=key==='app_fps'
    ? report.runs.flatMap(run=>(run.samples||[]).filter(s=>Number(s.window_s)>=.9).map(s=>({
        start_s:Number(s.unix_ms)/1000-Number(s.window_s),end_s:Number(s.unix_ms)/1000,
        app_fps:Number(s.frames)/Number(s.window_s),run:run.name})))
    : report.power_samples.filter(r=>r[key]!=null);
  const w=canvas.width=canvas.clientWidth*devicePixelRatio,h=canvas.height=230*devicePixelRatio,c=canvas.getContext('2d');
  c.scale(devicePixelRatio,devicePixelRatio);
  const W=w/devicePixelRatio,H=230;
  c.clearRect(0,0,W,H);c.font='12px system-ui';c.fillStyle='#e8f1f5';
  if(!rows.length){c.fillText('No samples for this metric',20,30);return;}
  const starts=report.runs.map(r=>r.start_s).filter(Number.isFinite),ends=report.runs.map(r=>r.end_s).filter(Number.isFinite);
  const first=starts.length?Math.min(...starts):rows[0].start_s,last=ends.length?Math.max(...ends):rows.at(-1).end_s;
  rows=rows.filter(r=>r.end_s>=first&&r.end_s<=last);
  const max=rows.reduce((m,r)=>Math.max(m,r[key]),1),
        x=t=>45+(t-first)/Math.max(last-first,.001)*(W-65),y=v=>H-30-v/max*(H-60);
  for(const run of report.runs){
    if(!run.start_s)continue;
    c.fillStyle=run.errors.length?'#773d3d55':'#3d777155';
    c.fillRect(x(Math.max(first,run.start_s)),20,x(Math.min(last,run.end_s))-x(Math.max(first,run.start_s)),H-50);
    c.fillStyle='#e8f1f5';c.fillText(run.name,x(Math.max(first,run.start_s))+3,15);
  }
  for(const r of report.power_samples){
    if(!['moderate','heavy'].includes(r.thermal)||r.end_s<first||r.start_s>last)continue;
    c.fillStyle=r.thermal==='heavy'?'#ed773c55':'#edc03c44';
    c.fillRect(x(Math.max(first,r.start_s)),20,x(Math.min(last,r.end_s))-x(Math.max(first,r.start_s)),H-50);
  }
  c.strokeStyle='#72d5c3';c.beginPath();
  rows.forEach((r,i)=>{const p=[x(r.end_s),y(r[key])];
    i&&r.run===rows[i-1].run&&r.start_s-rows[i-1].end_s<2?c.lineTo(...p):c.moveTo(...p);});
  c.stroke();c.fillStyle='#e8f1f5';c.fillText(max.toFixed(1),3,30);c.fillText('0',15,H-30);
  for(let i=0;i<=5;i++){const t=first+(last-first)*i/5;c.fillText(((t-first)/60).toFixed(1),x(t)-8,H-15);}
  c.fillText(starts.length?'Minutes from first measurement start':'Minutes from first plotted sample',45,H-1);
}
select.onchange=draw;window.onresize=draw;draw();</script>'''
    page = page.replace('<tbody>ROWS</tbody>', '<tbody>' + ''.join(table_rows) + '</tbody>', 1)
    page = page.replace('<p id="session">SESSION</p>', '<p id="session">' + html.escape(
        'Session: ' + manifest.get('status', 'unknown') + ('. ' + manifest['error'] if manifest.get('error') else '')) + '</p>', 1)
    page = page.replace('<pre>SUMMARY</pre>', '<pre>' + html.escape('\n'.join(lines)) + '</pre>', 1)
    (root / 'report.html').write_text(page.replace('const report=DATA;', 'const report=' + data + ';', 1))
    return report
