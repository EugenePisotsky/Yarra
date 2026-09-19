#!/usr/bin/env python3
"""Controlled grass profiling: run, ordered suites, and offline reports. Python standard library only."""
import argparse
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import sqlite3
import subprocess
import sys
import time

from grass_profile_report import status, write_report

ROOT = Path(__file__).resolve().parents[1]
DEFAULTS = dict(size='game', window='fullscreen', fps=60, msaa=4, warmup=20, seconds=90,
                view='low-walk', density='balanced', grass='full', counters=False,
                terrain_lod=True, native_pacing=False, prepass=False)
VIEWS = ['low-walk', 'grass-close', 'grass-away', 'grass-follow', 'grass-follow-far',
         'grass-zoom', 'grass-overhead', 'grass-top-down', 'grass-stream', 'grass-soak']
INPUTS = {'binary', 'world_db', 'shaders', 'canopy', 'vertex_reference', 'candidate_reference', 'placement_reference', 'terrain_reference', 'prepared_blades'}


def save(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def source(value, default):
    path = Path(value or default)
    return path.resolve() if path.is_absolute() else (ROOT / path).resolve()


def validate(settings):
    unknown = settings.keys() - DEFAULTS.keys() - INPUTS
    if unknown:
        raise ValueError(f'Unknown settings: {sorted(unknown)}')
    if settings['size'] != 'game':
        if not isinstance(settings['size'], str) or not re.fullmatch(r'\d+x\d+', settings['size']):
            raise ValueError('size must be game (normal world scale) or WIDTHxHEIGHT')
        if not all(64 <= int(n) <= 8192 for n in settings['size'].split('x')):
            raise ValueError('dimensions must be in 64..8192')
    if settings['window'] not in ['fullscreen', 'windowed']:
        raise ValueError('window must be fullscreen or windowed')
    if type(settings['fps']) is not int or settings['fps'] not in [0, *range(15, 241)]:
        raise ValueError('fps must be 0 (uncapped) or 15..240')
    for key, low, high in [('seconds', 2, 3600), ('warmup', 1, 600)]:
        value = settings[key]
        if type(value) not in (int, float) or not math.isfinite(value) or not low <= value <= high:
            raise ValueError(f'{key} must be in {low}..{high}')
    if type(settings['msaa']) is not int or settings['msaa'] not in [1, 2, 4] or settings['view'] not in VIEWS:
        raise ValueError('Invalid MSAA or view')
    if settings['density'] not in ['balanced', 'full', 'authored'] or settings['grass'] not in ['full', 'off']:
        raise ValueError('Invalid density or grass mode')
    for key in ['counters', 'terrain_lod', 'native_pacing', 'prepass',
                *[k for k in INPUTS if k.endswith('_reference')]]:
        if key in settings and not isinstance(settings[key], bool):
            raise ValueError(f'{key} must be boolean')
    for key in INPUTS - {k for k in INPUTS if k.endswith('_reference')}:
        if key == 'prepared_blades':
            value = settings.get(key)
            if value is not None and (type(value) is not int or not 32768 <= value <= 524288):
                raise ValueError('prepared_blades must be an integer in 32768..524288')
            continue
        if settings.get(key) is not None and not isinstance(settings[key], str):
            raise ValueError(f'{key} must be a path string')


def snapshot(destination, settings):
    destination.mkdir(parents=True)
    assets = destination / 'assets'
    assets.mkdir()
    for p in (ROOT / 'assets').iterdir():
        if p.name != 'shaders':
            (assets / p.name).symlink_to(p, target_is_directory=p.is_dir())
    shutil.copytree(ROOT / 'assets/shaders', assets / 'shaders')
    if settings.get('shaders'):
        shader_dir = source(settings['shaders'], '')
        if not shader_dir.is_dir():
            raise ValueError(f'No shader directory: {shader_dir}')
        for p in shader_dir.glob('*.wgsl'):
            shutil.copy2(p, assets / 'shaders' / p.name)
    binary = source(settings.get('binary'), 'target/release/yarra-app-game')
    canopy = source(settings.get('canopy'), 'content/vegetation/canopy-look.ron')
    database = source(settings.get('world_db'), 'assets/generated/world.runtime.sqlite')
    shutil.copy2(binary, destination / 'game')
    shutil.copy2(canopy, destination / 'canopy.ron')
    # SQLite backup includes a committed WAL, unlike copying only the main file.
    with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as src:
        with sqlite3.connect(destination / 'runtime.sqlite') as dst:
            src.backup(dst)
    metadata = {
        'binary_sha256': digest(destination / 'game'),
        'database_sha256': digest(destination / 'runtime.sqlite'),
        'canopy_sha256': digest(destination / 'canopy.ron'),
        'shader_sha256': {p.name: digest(p) for p in sorted((assets / 'shaders').glob('*.wgsl'))},
        'original_sources': {'binary': str(binary), 'database': str(database), 'canopy': str(canopy)},
        'asset_scope': 'Binary, DB, shaders and canopy copied; other assets linked to workspace and must remain unchanged',
    }
    save(destination / 'inputs.json', metadata)
    return metadata


def command(inputs, settings):
    result = [str(inputs / 'game'), '--world-db', str(inputs / 'runtime.sqlite'),
              '--canopy-look', str(inputs / 'canopy.ron'), '--render-repro', settings['view'],
              '--profile-seconds', str(settings['seconds']), '--profile-warmup', str(settings['warmup']),
              '--profile-size', settings['size'], '--profile-fps', str(settings['fps']),
              '--profile-window', settings['window'],
              '--profile-msaa', str(settings['msaa']), '--profile-grass', settings['grass'],
              '--grass-density', settings['density']]
    if settings.get('prepared_blades') is not None:
        result += ['--grass-prepared-blades', str(settings['prepared_blades'])]
    if settings['counters']:
        result.append('--grass-counters')
    if not settings['terrain_lod']:
        result.append('--terrain-legacy')
    for key, flag in [('native_pacing', '--profile-native-pacing'),
                      ('prepass', '--render-prepass')]:
        if settings[key]:
            result.append(flag)
    for option in ['vertex', 'candidate', 'placement']:
        if settings.get(f'{option}_reference'):
            result.append(f'--grass-{option}-reference')
    if settings.get('terrain_reference'):
        result.append('--terrain-reference')
    return result


def stop(process):
    if process is None or process.poll() is not None:
        return
    try:
        # Signal sudo itself, which forwards to the collector; never run a user-writable script as root.
        process.terminate()
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
    except PermissionError:
        print('Collector could not be stopped immediately; its finite sample limit will stop it.', file=sys.stderr)


class PowerCollector:
    def __init__(self, root, duration, enabled):
        self.root, self.duration, self.enabled = root, duration, enabled
        self.process = self.output = self.errors = None

    def start(self):
        if not self.enabled:
            return
        if sys.platform != 'darwin':
            raise RuntimeError('powermetrics requires macOS; use --power off on other platforms')
        print('Power telemetry needs one local sudo authentication. Only /usr/bin/powermetrics runs as root.', flush=True)
        # Inherit the real terminal. Never read, store or pipe the password ourselves.
        if subprocess.run(['sudo', '-n', '-v'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            if not sys.stdin.isatty():
                raise RuntimeError('Run this command in a local terminal to authenticate sudo, or use --power off')
            subprocess.run(['sudo', '-v'], check=True)
        self.output = (self.root / 'power.txt').open('w')
        self.errors = (self.root / 'power-stderr.txt').open('w')
        cmd = ['sudo', '-n', '--', '/usr/bin/powermetrics', '--samplers', 'cpu_power,gpu_power,thermal',
               '--sample-rate', '1000', '--sample-count', str(math.ceil(self.duration)), '--buffer-size', '1']
        save(self.root / 'power-command.json', cmd)
        self.process = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=self.output, stderr=self.errors)

    def check(self):
        if self.process is not None and self.process.poll() is not None:
            raise RuntimeError(f'Power collector stopped early ({self.process.returncode}); inspect power-stderr.txt')

    def close(self):
        stop(self.process)
        for f in (self.output, self.errors):
            if f:
                f.close()


class MacmonCollector(PowerCollector):
    """Optional rootless collector. The caller supplies an existing executable."""
    def __init__(self, root, duration, executable):
        super().__init__(root, duration, True)
        self.executable = executable

    def start(self):
        if sys.platform != 'darwin':
            raise RuntimeError('macmon requires macOS')
        found = shutil.which(self.executable)
        if not found:
            raise RuntimeError('macmon not found; supply --macmon PATH or use --power required/off')
        executable = Path(found).resolve()
        version = subprocess.run([str(executable), '--version'], check=True,
                                 capture_output=True, text=True, timeout=10).stdout.strip()
        cmd = [str(executable), 'pipe', '--samples', str(math.ceil(self.duration)), '--interval', '1000']
        save(self.root / 'power-collector.json', {
            'kind': 'macmon', 'version': version, 'sha256': digest(executable),
            'command': cmd, 'interval_ms': 1000,
            'source': 'https://github.com/vladkens/macmon',
            'scope': 'System-wide private-API estimates; not process-attributed power',
        })
        print(f'Rootless power telemetry: {version} (no sudo).', flush=True)
        self.output = (self.root / 'power-macmon.jsonl').open('w')
        self.errors = (self.root / 'power-stderr.txt').open('w')
        self.process = subprocess.Popen(cmd, stdin=subprocess.DEVNULL,
                                        stdout=self.output, stderr=self.errors)


def wait(seconds, collector):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        collector.check()
        time.sleep(min(1, max(0, end - time.monotonic())))


def execute_run(root, index, name, settings, input_dir, input_meta, collector):
    output = root / 'runs' / f'{index:02}-{name}'
    output.mkdir(parents=True)
    (output / 'assets').symlink_to(input_dir / 'assets', target_is_directory=True)
    cmd = command(input_dir, settings)
    metadata = {'name': f'{index:02}-{name}', 'settings': settings, 'inputs': input_meta,
                'command': cmd, 'power_required': collector.enabled,
                'local_utc_offset_seconds': dt.datetime.now().astimezone().utcoffset().total_seconds()}
    save(output / 'run.json', metadata)
    print(f"{metadata['name']}: {settings['window']}, internal size {settings['size']}, {settings['fps']} fps, warmup {settings['warmup']}s + measurement {settings['seconds']}s", flush=True)
    process = None
    try:
        env = dict(os.environ, MTL_HUD_ENABLED='1', MTL_HUD_LOG_ENABLED='1', NO_COLOR='1', RUST_LOG='warn')
        for key in ['MTL_CAPTURE_ENABLED', 'METAL_DEVICE_WRAPPER_TYPE', 'MTL_DEBUG_LAYER', 'MTL_SHADER_VALIDATION']:
            env.pop(key, None)
        with (output / 'game.log').open('w') as log:
            process = subprocess.Popen(cmd, cwd=output, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + settings['warmup'] + settings['seconds'] + 60
            while process.poll() is None:
                collector.check()
                if time.monotonic() > deadline:
                    raise RuntimeError('Game watchdog expired; partial artifacts preserved')
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    pass
            metadata['exit_code'] = process.returncode
            if process.returncode:
                raise RuntimeError(f"Game exited {process.returncode}; inspect {output / 'game.log'}")
    finally:
        stop(process)
        metadata['exit_code'] = process.returncode if process else None
        save(output / 'run.json', metadata)


def check_other_games():
    # Advisory read only: never terminate or suspend the user's processes.
    result = subprocess.run(['ps', '-axo', 'pid=,comm='], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError('Cannot check competing game processes; run from a local terminal')
    matches = [line.strip() for line in result.stdout.splitlines()
               if re.search(r'/(yarra-app-game|yarra-app-editor|YarraReleaseProfile|game)$', line.strip())]
    if matches:
        raise RuntimeError('Close the existing game/editor before profiling: ' + ', '.join(matches))


def host_context():
    result = {'os': platform.platform(), 'architecture': platform.machine()}
    if sys.platform == 'darwin':
        for name, cmd in {
            'model': ['/usr/sbin/sysctl', '-n', 'hw.model'],
            'cpu': ['/usr/sbin/sysctl', '-n', 'machdep.cpu.brand_string'],
            'memory_bytes': ['/usr/sbin/sysctl', '-n', 'hw.memsize'],
            'power_source': ['/usr/bin/pmset', '-g', 'batt'],
        }.items():
            captured = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
            result[name] = captured.stdout.strip() if captured.returncode == 0 else None
    return result


def run_session(args, variants, order, idle, gap):
    if hasattr(os, 'geteuid') and os.geteuid() == 0:
        raise RuntimeError('Run the tool as your normal user, not with sudo; it elevates only powermetrics')
    for name, config in variants.items():
        if not re.fullmatch(r'[A-Za-z0-9_-]+', name):
            raise ValueError('Variant names may contain letters, numbers, underscore and hyphen')
        validate(config)
    if not order or len(order) > 32 or any(name not in variants for name in order):
        raise ValueError('order must contain 1..32 existing variant names')
    if not all(isinstance(v, (int, float)) and math.isfinite(v) and 0 <= v <= 600 for v in (idle, gap)):
        raise ValueError('idle/gap must be 0..600 seconds')
    check_other_games()
    if not args.no_build and any(not v.get('binary') for v in variants.values()):
        subprocess.run(['cargo', 'build', '--offline', '--locked', '--release', '-p', 'yarra-app-game'], cwd=ROOT, check=True)
    root = (args.output or ROOT / 'tmp/grass-profiles' / dt.datetime.now().strftime('%Y%m%d-%H%M%S')).resolve()
    root.mkdir(parents=True, exist_ok=False)
    snapshots = {name: snapshot(root / 'inputs' / name, config) for name, config in variants.items()}
    session = {'version': 1, 'started': dt.datetime.now().astimezone().isoformat(),
               'variants': variants, 'order': order, 'idle_seconds': idle, 'gap_seconds': gap,
               'power': args.power, 'status': 'running', 'platform': sys.platform,
               'host': host_context(),
               'git_head': subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True).stdout.strip(),
               'git_status': subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True).stdout}
    save(root / 'session.json', session)
    # One bounded collector spans all runs; no authentication between variants.
    total = idle + sum(variants[n]['warmup'] + variants[n]['seconds'] + gap + 65 for n in order) + 30
    collector = (MacmonCollector(root, total, args.macmon) if args.power == 'macmon'
                 else PowerCollector(root, total, args.power == 'required'))
    try:
        collector.start()
        print(f'Artifacts: {root}\nKeep the game focused; do not change assets/settings or run other GPU work.', flush=True)
        begin = time.time()
        wait(idle, collector)
        session['idle_window'] = [begin, time.time()]
        for i, name in enumerate(order, 1):
            execute_run(root, i, name, variants[name], root / 'inputs' / name, snapshots[name], collector)
            if i != len(order):
                print(f'Inter-run idle: {gap}s (a gap does not guarantee thermal recovery)', flush=True)
                wait(gap, collector)
        session['status'] = 'complete'
    except BaseException as error:
        session['status'] = 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed'
        session['error'] = str(error)
        raise
    finally:
        collector.close()
        save(root / 'session.json', session)
        report = write_report(root)
        print(f"Report: {root / 'report.html'}", flush=True)
        for run in report['runs']:
            print(run['name'], status(run),
                  '; '.join(run['errors'] + run['target_misses'] + run['warnings']), flush=True)
    if any(run['errors'] for run in report['runs']):
        raise RuntimeError('One or more runs failed validity checks; inspect the report')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='action', required=True)
    report = subs.add_parser('report', help='Regenerate JSON, Markdown and HTML reports without sudo')
    report.add_argument('directory', type=Path)
    for name in ('run', 'suite'):
        p = subs.add_parser(name)
        p.add_argument('--output', type=Path)
        p.add_argument('--power', choices=['required', 'macmon', 'off'], default='required',
                       help='required = sudo powermetrics; macmon = rootless collector; off = no power telemetry')
        p.add_argument('--macmon', default='macmon', help='Existing macmon executable for --power macmon')
        p.add_argument('--no-build', action='store_true', help='Use existing release binary; hashes remain recorded')
        if name == 'suite':
            p.add_argument('config', type=Path)
        else:
            p.add_argument('--name', default='current')
            p.add_argument('--idle', type=float, default=10)
            for key, default in DEFAULTS.items():
                if key == 'window':
                    p.add_argument('--window', choices=['fullscreen', 'windowed'], default=default,
                                   help='Presentation mode (default: fullscreen on the primary display)')
                elif key == 'size':
                    p.add_argument('--size', default=default,
                                   help='game = normal world scale (default); WIDTHxHEIGHT = fixed internal pixels')
                elif key == 'terrain_lod':
                    p.add_argument('--terrain-legacy', dest=key, action='store_false', default=default,
                                   help='Use the old local terrain renderer for a diagnostic comparison')
                elif isinstance(default, bool):
                    p.add_argument('--' + key.replace('_', '-'), action='store_true')
                else:
                    p.add_argument('--' + key.replace('_', '-'), type=type(default), default=default)
            for key in sorted(INPUTS):
                options = {'action': 'store_true'} if key.endswith('_reference') else {'action': 'store'}
                if key == 'prepared_blades':
                    options['type'] = int
                p.add_argument('--' + key.replace('_', '-'), **options)
    args = parser.parse_args()
    if args.action == 'report':
        write_report(args.directory)
        print(args.directory / 'report.html')
    elif args.action == 'suite':
        spec = json.loads(args.config.read_text())
        if not isinstance(spec, dict) or not isinstance(spec.get('defaults', {}), dict):
            raise ValueError('Suite and defaults must be JSON objects')
        if not isinstance(spec.get('variants'), dict) or not all(isinstance(v, dict) for v in spec['variants'].values()):
            raise ValueError('variants must map names to settings objects')
        if not isinstance(spec.get('order'), list):
            raise ValueError('order must be a list of variant names')
        unknown = spec.keys() - {'defaults', 'variants', 'order', 'idle_seconds', 'gap_seconds'}
        if unknown:
            raise ValueError(f'Unknown suite keys: {sorted(unknown)}')
        variants = {name: DEFAULTS | spec.get('defaults', {}) | values for name, values in spec['variants'].items()}
        run_session(args, variants, spec['order'], spec.get('idle_seconds', 10), spec.get('gap_seconds', 15))
    else:
        config = {k: getattr(args, k) for k in DEFAULTS.keys() | INPUTS}
        run_session(args, {args.name: config}, [args.name], args.idle, 0)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, ValueError, OSError, subprocess.SubprocessError) as error:
        print(f'Error: {error}', file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print('Interrupted; partial artifacts preserved.', file=sys.stderr)
        sys.exit(130)
