"""Regression checks for terrain height packing, using small synthetic scan maps."""
import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image

from import_terrain_surfaces import import_surface


class TerrainBlendHeightImportTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sources = self.root / 'sources'
        scan = self.sources / 'scan'
        scan.mkdir(parents=True)
        y, x = np.mgrid[:64, :64]
        height = np.rint((np.sin(x * np.pi / 16) + np.cos(y * np.pi / 16) + 2) * 63.75)
        Image.fromarray(height.astype(np.uint8)).save(scan / 'scan_displacement.png')
        Image.new('RGB', (64, 64), (120, 100, 70)).save(scan / 'scan_basecolor.png')
        Image.new('RGB', (64, 64), (128, 128, 255)).save(scan / 'scan_normal.png')
        Image.new('L', (64, 64), 230).save(scan / 'scan_ao.png')
        Image.new('L', (64, 64), 175).save(scan / 'scan_roughness.png')

    def imported(self, name, **parameters):
        output = self.root / name
        report = import_surface(
            {'key': 'ground', 'source': 'scan', **parameters}, self.sources, output, 32)
        with Image.open(output / 'ground/base_color.png') as image:
            color = np.array(image)
        normal = (output / 'ground/normal_material.png').read_bytes()
        return color, normal, report

    def test_pack_strength_scales_alpha_only_and_reimport_does_not_accumulate(self):
        full, normal, _ = self.imported('full', blend_height=1.0)
        quarter, quarter_normal, report = self.imported('quarter', blend_height=0.25)
        zero, zero_normal, _ = self.imported('zero', blend_height=0.0)
        self.assertEqual(int(full[..., 3].min()), 0)
        self.assertEqual(int(full[..., 3].max()), 255)
        self.assertEqual(int(quarter[..., 3].max()), 64)
        self.assertTrue(np.all(np.abs(quarter[..., 3] - full[..., 3] * 0.25) <= 0.625))
        self.assertTrue(np.all(zero[..., 3] == 0))
        np.testing.assert_array_equal(full[..., :3], quarter[..., :3])
        np.testing.assert_array_equal(full[..., :3], zero[..., :3])
        self.assertEqual(normal, quarter_normal)
        self.assertEqual(normal, zero_normal)
        self.assertEqual(report['blend_height'], 0.25)
        again, _, _ = self.imported('quarter', blend_height=0.25)
        np.testing.assert_array_equal(quarter, again)

    def test_omitted_strength_preserves_full_height(self):
        default, _, report = self.imported('default')
        full, _, _ = self.imported('full', blend_height=1.0)
        np.testing.assert_array_equal(default, full)
        self.assertEqual(report['blend_height'], 1.0)

    def test_invalid_strength_is_rejected_before_writing(self):
        for strength in (-0.1, 1.1, float('nan'), float('inf')):
            with self.subTest(strength=strength):
                with self.assertRaisesRegex(ValueError, 'blend_height'):
                    self.imported('invalid', blend_height=strength)
                self.assertFalse((self.root / 'invalid').exists())


if __name__ == '__main__':
    unittest.main()
