"""Reanalyze existing evidence only. No game launch or power collection."""
import hashlib
import importlib.util
import json
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('report_tools', ROOT / 'tools/grass_profile_report.py')
analyzer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(analyzer)


def analyze():
    results = []
    for session, roots, capacity in (('20260916-200703', 66, 131072),
                                     ('20260916-212946', 96, 524288),
                                     ('20260916-220712', 96, 131072)):
        source = ROOT / 'docs/performance' / session
        report_bytes = (source / 'report.json').read_bytes()
        report = json.loads(report_bytes)
        run = report['runs'][0]
        log_path = f"runs/{run['name']}/game.log"
        with zipfile.ZipFile(source / 'evidence.zip') as archive:
            log = archive.read(log_path)
        lines = log.decode().splitlines()
        events = [analyzer.fields(line) for line in lines if 'GRASS_PROFILE ' in line]
        config = next(e for e in events if e.get('event') == 'config')
        launch = int(config['unix_ms']) / 1000
        onset = next(p for p in report['power_samples']
                     if p['end_s'] >= launch and p['thermal'] not in (None, 'nominal'))
        audits = [analyzer.fields(line) for line in lines if 'RENDER_AUDIT ' in line]
        ready = next(a for a in audits if a.get('source_pages') == '49'
                     and a.get('terrain_prepared_active') == '49')
        results.append({
            'session': session, 'roots_m2': roots, 'prepared_blades': capacity,
            'report_sha256': hashlib.sha256(report_bytes).hexdigest(),
            'raw_log_sha256': hashlib.sha256(log).hexdigest(),
            'game_config_unix_s': launch,
            'measurement_start_after_config_s': run['start_s'] - launch,
            'first_ready_audit_game_elapsed_s': float(ready['elapsed_s']),
            'first_elevated_pressure_after_config_s': onset['end_s'] - launch,
            'windows': [{'game_elapsed_from_s': lo, 'game_elapsed_to_s': hi,
                         'power': analyzer.power_window(report['power_samples'], launch + lo, launch + hi)}
                        for lo, hi in ((0, 60), (60, 240), (0, 240), (120, 240))],
        })
    return {'experiment': 'GP-021',
            'parser_sha256': hashlib.sha256((ROOT / 'tools/grass_profile_report.py').read_bytes()).hexdigest(),
            'method': 'Whole power intervals, same archived parser logic; epochs from raw game config. No new measurements. First 60 seconds are stationary camera/wind asset warmup, not full moving-workload thermal warmup.',
            'runs': results}


if __name__ == '__main__':
    path = Path(__file__).with_name('analysis.json')
    path.write_text(json.dumps(analyze(), indent=2, allow_nan=False) + '\n')
    print(path)
