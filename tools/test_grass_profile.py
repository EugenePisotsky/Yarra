"""Run with: python3 -m unittest discover -s tools -p 'test_grass_profile.py'."""
import datetime as dt
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

import grass_profile as runner
import grass_profile_report as report


def power_sample(end, duration=1, cpu=2000, gpu=4000):
    date = dt.datetime.fromtimestamp(end, dt.timezone.utc).strftime('%a %b %d %H:%M:%S %Y %z')
    return (f'*** Sampled system activity ({date}) ({duration * 1000}ms elapsed) ***\n'
            f'CPU Power: {cpu} mW\nGPU Power: {gpu} mW\n'
            'Current pressure level: Nominal\nGPU HW active frequency: 900 MHz\n'
            'GPU HW active residency:  70.00%\nGPU Power: 99999 mW\n')


class PowerTests(unittest.TestCase):
    def test_macmon_uses_active_ratio_and_excludes_missing_time(self):
        def sample(second, **extra):
            return json.dumps(dict(timestamp=f'1970-01-01T00:01:{second:02}+00:00',
                                   gpu_power=4, cpu_power=2, gpu_freq_mhz=900,
                                   gpu_active_ratio=.7, gpu_usage=[900, .4],
                                   temp={'gpu_temp_avg': 65}, fans=[{'rpm': 2500}], **extra))
        rows = report.parse_macmon('\n'.join(sample(s) for s in [40, 41, 42, 50, 51]))
        self.assertEqual([r['end_s'] for r in rows], [101, 102, 111])
        result = report.power_window(rows, 100.5, 112)
        self.assertEqual(result['coverage_s'], 2)
        self.assertEqual(result['gpu_active_percent'], 70)
        self.assertEqual(result['gpu_w'], 4)
        self.assertEqual(result['gpu_temp_c'], 65)
        self.assertEqual(result['fan0_rpm'], 2500)
        self.assertIsNone(result['fan1_rpm'])
        self.assertEqual(result['thermal_states'], [])

    def test_macmon_missing_or_malformed_data_does_not_invent_telemetry(self):
        data = {'timestamp': '1970-01-01T00:01:40+00:00'}
        first = json.dumps(data)
        data.update(timestamp='1970-01-01T00:01:41+00:00', gpu_power=float('nan'),
                    gpu_active_ratio=1.5, cpu_power=True, gpu_usage=[900, .4])
        rows = report.parse_macmon(first + '\n' + json.dumps(data))
        self.assertEqual(len(rows), 1)
        self.assertTrue(all(rows[0][k] is None for k in ('gpu_w', 'cpu_w', 'gpu_active_percent', 'gpu_mhz')))
        self.assertEqual(report.parse_macmon(first + '\n{truncated\n' + json.dumps(data)), [])

    def test_duplicate_gpu_sections_and_missing_values(self):
        text = power_sample(102) + power_sample(104, 2, gpu=8000)
        rows = report.parse_power(text)
        self.assertEqual([r['gpu_w'] for r in rows], [4, 8])
        summary = report.power_window(rows, 100, 105)
        self.assertAlmostEqual(summary['gpu_w'], 20 / 3)
        self.assertEqual(summary['coverage_s'], 3)
        self.assertEqual(summary['thermal_states'], ['nominal'])
        missing = report.parse_power(power_sample(102).replace('GPU Power:', 'Missing:'))
        self.assertIsNone(report.power_window(missing, 100, 105)['cpu_gpu_w'])
        self.assertIsNone(report.power_window([], 100, 105)['gpu_w'])

    def test_boundary_samples_are_excluded(self):
        rows = report.parse_power(power_sample(101) + power_sample(102) + power_sample(103))
        summary = report.power_window(rows, 100.5, 102.5)
        self.assertEqual(summary['samples'], 1)
        self.assertEqual(summary['coverage_s'], 1)

    def test_saved_timezone_keeps_report_portable(self):
        self.assertEqual(report.timestamp('1970-01-01 03:01:41.200 game metal-HUD:', 10800), 101.2)


class RunTests(unittest.TestCase):
    def test_native_lod_soak_flags_must_reach_the_measured_run(self):
        self.meta['settings'].update(terrain_lod=True, native_pacing=True, prepass=True,
                                     view='grass-soak')
        cmd = runner.command(Path('/inputs'), self.meta['settings'])
        for flag in ('--terrain-lod', '--profile-native-pacing', '--render-prepass'):
            self.assertIn(flag, cmd)
        self.assertEqual(len(self.analyze()['errors']), 3)
        self.log = ('GRASS_PROFILE event=config pacing=native\n' + self.log
                    .replace('terrain_lod=false', 'terrain_lod=true')
                    .replace('prepass=false', 'prepass=true'))
        self.assertEqual(self.analyze()['errors'], [])

    def test_preparation_experiment_is_validated_and_must_be_applied(self):
        settings = runner.DEFAULTS | {'prepared_blades': 524288}
        runner.validate(settings)
        cmd = runner.command(Path('/inputs'), settings)
        self.assertEqual(cmd[cmd.index('--grass-prepared-blades') + 1], '524288')
        for invalid in [True, '524288', 0, 524289]:
            with self.assertRaises(ValueError):
                runner.validate(runner.DEFAULTS | {'prepared_blades': invalid})

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.meta = dict(name='baseline', inputs={}, settings=runner.DEFAULTS | {'size': '2560x1440', 'window': 'windowed'},
                         exit_code=0, power_required=False, local_utc_offset_seconds=0)
        self.audit = ('RENDER_AUDIT unix_ms=105000 render_px=2560x1440 msaa_samples=4 density=Balanced '
                      'surface_px=2560x1440 scale=1 window_mode=windowed '
                      'grass=full counters=false prepass=false terrain_lod=false thermal=nominal source_revision=1 '
                      'terrain_prepared_pages=49 terrain_prepared_active=49 sampled_capacity_drops=[0, 0, 0, 0]')
        self.log = '\n'.join([
            'GRASS_PROFILE event=measure_start unix_ms=100000 focused=true',
            '1970-01-01 00:01:39.900 game metal-HUD: 1,0,0,16.67,99,16.67,99',
            '1970-01-01 00:01:40.200 game metal-HUD: 2,0,0,16.67,50,16.67,50',
            '1970-01-01 00:01:41.200 game metal-HUD: 3,0,0,16.67,4,16.67,4',
            '1970-01-01 00:01:41.200 game metal-HUD: 3,0,0,16.67,4,16.67,4',
            self.audit,
            'GRASS_PROFILE event=sample unix_ms=110000 frames=600 window_s=10 late_updates=0 '
            'update_max_ms=18 update_p95_ms=17 update_p99_ms=17.5 focused=true',
            'GRASS_PROFILE event=complete unix_ms=110000 measured_s=10 frames=600',
        ])

    def analyze(self, power_rows=()):
        runner.save(self.root / 'run.json', self.meta)
        (self.root / 'game.log').write_text(self.log)
        return report.analyze_run(self.root, power_rows)

    def test_warmup_hud_excluded_and_duplicates_not_counted(self):
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['target_misses'], [])
        self.assertEqual(result['updates']['fps'], 60)
        self.assertEqual(result['hud']['samples'], 2)
        self.assertEqual(result['hud']['gpu_mean_ms'], 4)
        self.assertIsNone(result['power']['gpu_w'])
        self.assertEqual(report.status(result), 'CHECK')

    def test_old_binary_cannot_silently_ignore_preparation_override(self):
        self.meta['settings']['prepared_blades'] = 524288
        self.assertTrue(any('blade_preparation_bytes' in e for e in self.analyze()['errors']))
        self.log = self.log.replace('RENDER_AUDIT ', 'RENDER_AUDIT blade_preparation_bytes=70516748 ')
        self.assertEqual(self.analyze()['errors'], [])

    def test_missed_target_is_a_finding_not_an_invalid_run(self):
        self.log = self.log.replace('frames=600', 'frames=400')
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertEqual(report.status(result), 'MISSED TARGET')

    def test_long_run_mean_does_not_hide_brief_cadence_loss(self):
        self.meta['settings']['fps'] = 120
        self.log = self.log.replace('16.67,', '8.33,')
        sample_start = self.log.index('GRASS_PROFILE event=sample')
        self.log = self.log[:sample_start] + '\n'.join(
            f'GRASS_PROFILE event=sample unix_ms={101000 + i * 1000} '
            f'frames={80 if i == 80 else 120} window_s=1 late_updates={40 if i == 80 else 0} '
            'update_max_ms=17 update_p95_ms=8.5 update_p99_ms=9 focused=true'
            for i in range(100)) + '\nGRASS_PROFILE event=complete unix_ms=200000 measured_s=100 frames=11960'
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['target_misses'], [])
        self.assertEqual(result['updates']['fps'], 119.6)
        self.assertEqual(result['updates']['worst_window_fps'], 80)
        self.assertEqual(result['updates']['worst_window_end_s'], 81)
        self.assertEqual(result['updates']['below_95pct_target_windows'], 1)
        self.assertTrue(any('cadence loss' in w for w in result['warnings']))
        self.assertEqual(report.status(result), 'CHECK')

    def test_short_final_sample_does_not_create_a_cadence_warning(self):
        self.log = self.log.replace('GRASS_PROFILE event=complete',
            'GRASS_PROFILE event=sample unix_ms=110100 frames=4 window_s=0.1 '
            'late_updates=0 update_max_ms=17 update_p95_ms=17 update_p99_ms=17 focused=true\n'
            'GRASS_PROFILE event=complete')
        result = self.analyze()
        self.assertEqual(result['updates']['worst_window_fps'], 60)
        self.assertEqual(result['updates']['below_95pct_target_windows'], 0)
        self.assertFalse(any('cadence loss' in w for w in result['warnings']))

    def test_average_60_with_alternating_present_intervals_warns_about_pacing(self):
        payload = '3,0,0,' + ','.join(['8.33,4,25.0,4'] * 20)
        self.log = self.log.replace('3,0,0,16.67,4,16.67,4', payload)
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['target_misses'], [])
        self.assertAlmostEqual(result['hud']['presentation_interval_mean_ms'], 16.665)
        self.assertEqual(result['hud']['presentation_interval_p95_ms'], 25)
        self.assertEqual(result['hud']['presentation_tail_sample_fraction'], .5)
        self.assertTrue(any('frame pacing' in w for w in result['warnings']))
        self.assertEqual(report.status(result), 'CHECK')

    def test_focus_resolution_capacity_and_partial_run_fail(self):
        self.log = self.log.replace('focused=true', 'focused=false').replace('2560x1440', '1920x1080')
        self.log = self.log.replace('[0, 0, 0, 0]', '[0, 1, 0, 0]')
        result = self.analyze()
        self.assertEqual(len(result['errors']), 3)
        self.assertEqual(report.status(result), 'INVALID')
        self.meta['exit_code'] = -15
        self.log = self.log.replace('event=complete', 'event=interrupted')
        result = self.analyze()
        self.assertTrue(any('partial' in e for e in result['errors']))

    def test_required_power_needs_metric_coverage(self):
        self.meta['power_required'] = True
        power = report.parse_power(''.join(power_sample(i) for i in range(101, 111)))
        self.assertEqual(self.analyze(power)['errors'], [])
        for row in power[:5]:
            row['gpu_mhz'] = None
        self.assertTrue(any('telemetry unavailable' in e for e in self.analyze(power)['errors']))

    def test_fullscreen_normal_scale_checks_observed_surface_and_world_size(self):
        self.meta['settings'].update(size='game', window='fullscreen')
        self.log = self.log.replace('render_px=2560x1440', 'render_px=2592x1675')
        self.log = self.log.replace('surface_px=2560x1440 scale=1 window_mode=windowed',
                                    'surface_px=3456x2234 scale=0.75 window_mode=fullscreen')
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertEqual(result['display']['render_px'], '2592x1675')
        self.assertEqual(result['display']['surface_px'], '3456x2234')
        self.log = self.log.replace('render_px=2592x1675', 'render_px=1920x1080')
        self.assertTrue(any('normal game scale' in e for e in self.analyze()['errors']))

    def test_fullscreen_requested_but_windowed_or_unreported_is_invalid(self):
        self.meta['settings']['window'] = 'fullscreen'
        self.assertTrue(any('window_mode' in e for e in self.analyze()['errors']))
        self.log = self.log.replace('window_mode=windowed', '')
        self.assertTrue(any('window_mode' in e for e in self.analyze()['errors']))

    def test_legacy_report_without_window_mode_remains_readable(self):
        del self.meta['settings']['window']
        self.log = self.log.replace('window_mode=windowed', '')
        result = self.analyze()
        self.assertEqual(result['errors'], [])
        self.assertIsNone(result['display']['window_mode'])

    def test_offline_report_handles_interrupted_artifacts(self):
        directory = self.root / 'runs/01-partial'
        directory.mkdir(parents=True)
        runner.save(directory / 'run.json', self.meta | {'exit_code': None})
        runner.save(self.root / 'session.json', {'status': 'interrupted'})
        result = report.write_report(self.root)
        self.assertEqual(report.status(result['runs'][0]), 'INVALID')
        for suffix in ('json', 'md', 'html'):
            self.assertTrue((self.root / f'report.{suffix}').is_file())


class RunnerTests(unittest.TestCase):
    def test_rootless_collector_records_identity_and_never_calls_sudo(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(runner.sys, 'platform', 'darwin'), \
                patch.object(runner.subprocess, 'run', return_value=Mock(stdout='macmon 0.8.2')) as run, \
                patch.object(runner.subprocess, 'Popen') as popen:
            root = Path(tmp)
            binary = (root / 'macmon').resolve()
            binary.write_text('fixture executable')
            binary.chmod(0o755)
            popen.return_value.poll.return_value = 0
            collector = runner.MacmonCollector(root, 100.2, str(binary))
            collector.start()
            collector.close()
            self.assertEqual(run.call_args.args[0], [str(binary), '--version'])
            self.assertEqual(popen.call_args.args[0],
                             [str(binary), 'pipe', '--samples', '101', '--interval', '1000'])
            meta = json.loads((root / 'power-collector.json').read_text())
            self.assertEqual(meta['sha256'], runner.digest(binary))
            self.assertEqual(meta['version'], 'macmon 0.8.2')
            with self.assertRaisesRegex(RuntimeError, 'stopped early'):
                collector.check()

    def test_missing_rootless_collector_fails_without_sudo_fallback(self):
        with patch.object(runner.sys, 'platform', 'darwin'), \
                patch.object(runner.shutil, 'which', return_value=None), \
                patch.object(runner.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'macmon not found'):
                runner.MacmonCollector(Path('/unused'), 60, 'macmon').start()
            run.assert_not_called()

    def test_invalid_settings_rejected_before_launch(self):
        for change in ({'fps': True}, {'fps': 60.0}, {'warmup': float('nan')}, {'msaa': True},
                       {'size': '0x1440'}, {'size': 1440}, {'window': 'maximized'}, {'counters': 1},
                       {'terrain_lod': 1}, {'native_pacing': 'true'}, {'prepass': None},
                       {'binary': []}, {'density': 'ultra'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                runner.validate(runner.DEFAULTS | change)

    def test_fullscreen_command_and_historical_suite_settings(self):
        runner.validate(runner.DEFAULTS)
        cmd = runner.command(Path('/inputs'), runner.DEFAULTS)
        self.assertEqual(cmd[cmd.index('--profile-window') + 1], 'fullscreen')
        self.assertEqual(cmd[cmd.index('--profile-size') + 1], 'game')
        for name in ('grass-60-vs-120', 'grass-on-off'):
            spec = json.loads((runner.ROOT / 'tools/profiles' / f'{name}.json').read_text())
            for variant in spec['variants'].values():
                settings = runner.DEFAULTS | spec['defaults'] | variant
                runner.validate(settings)
                self.assertEqual((settings['window'], settings['size']), ('windowed', '2560x1440'))

    def test_single_bounded_native_collector_and_no_password_pipe(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(runner.sys, 'platform', 'darwin'), \
                patch.object(runner.subprocess, 'run', return_value=Mock(returncode=0)) as run, \
                patch.object(runner.subprocess, 'Popen') as popen:
            popen.return_value.poll.return_value = 0
            collector = runner.PowerCollector(Path(tmp), 100.2, True)
            collector.start()
            collector.check = Mock()  # Collector is mocked as already exited for cleanup.
            collector.close()
            self.assertEqual(run.call_count, 1)
            self.assertEqual(run.call_args.args[0], ['sudo', '-n', '-v'])
            self.assertEqual(popen.call_count, 1)
            cmd = popen.call_args.args[0]
            self.assertEqual(cmd[:4], ['sudo', '-n', '--', '/usr/bin/powermetrics'])
            self.assertEqual(cmd[cmd.index('--sample-count') + 1], '101')
            self.assertEqual(popen.call_args.kwargs['stdin'], subprocess.DEVNULL)

    def test_missing_noninteractive_auth_stops_before_collector(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(runner.sys, 'platform', 'darwin'), \
                patch.object(runner.sys.stdin, 'isatty', return_value=False), \
                patch.object(runner.subprocess, 'run', return_value=Mock(returncode=1)), \
                patch.object(runner.subprocess, 'Popen') as popen:
            with self.assertRaisesRegex(RuntimeError, 'local terminal'):
                runner.PowerCollector(Path(tmp), 60, True).start()
            popen.assert_not_called()

    def test_power_off_does_not_invoke_sudo(self):
        with patch.object(runner.subprocess, 'run') as run, patch.object(runner.subprocess, 'Popen') as popen:
            collector = runner.PowerCollector(Path('/unused'), 60, False)
            collector.start()
            collector.check()
            collector.close()
            run.assert_not_called()
            popen.assert_not_called()

    def test_child_cleanup(self):
        process = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        try:
            runner.stop(process)
            self.assertIsNotNone(process.poll())
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_snapshot_includes_committed_wal_and_canopy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'assets/shaders').mkdir(parents=True)
            (root / 'assets/shaders/grass.wgsl').write_text('shader')
            (root / 'game').write_text('binary')
            (root / 'look.ron').write_text('look')
            db = sqlite3.connect(root / 'world.sqlite')
            self.addCleanup(db.close)
            db.execute('pragma journal_mode=wal')
            db.execute('create table density (value integer)')
            db.execute('insert into density values (72)')
            db.commit()
            settings = dict(binary='game', canopy='look.ron', world_db='world.sqlite')
            with patch.object(runner, 'ROOT', root):
                meta = runner.snapshot(root / 'snapshot', settings)
            with sqlite3.connect(root / 'snapshot/runtime.sqlite') as copy:
                self.assertEqual(copy.execute('select value from density').fetchone(), (72,))
            self.assertEqual(meta['canopy_sha256'], runner.digest(root / 'look.ron'))
            self.assertEqual((root / 'snapshot/assets/shaders/grass.wgsl').read_text(), 'shader')
            db.close()


if __name__ == '__main__':
    unittest.main()
