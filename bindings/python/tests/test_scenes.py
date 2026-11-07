"""The default asset is shared with the web build and retains sensor radiance."""
import math
import struct

import pytest

from fujimod import scenes
from fujimod._native import emulator


def test_default_scene_is_sized_and_reused():
    asset = scenes.default_scene_raf()
    assert asset.stat().st_size == scenes._DEFAULT_RAF_SIZE
    modified = asset.stat().st_mtime_ns
    assert scenes.default_scene_raf() == asset
    assert asset.stat().st_mtime_ns == modified


def test_default_raf_retains_linear_color_in_its_visible_sensor_crop():
    width, height, data = emulator.raf_radiance_rgb(scenes.default_scene_raf().read_bytes())
    assert (width, height) == (816, 544)
    assert len(data) == width * height * 3 * 4
    samples = struct.unpack(f'<{width*height*3}f', data)
    assert all(math.isfinite(value) and value >= 0 for value in samples)
    assert max(samples) > 16383  # Exposure-normalized radiance, not clipped ADC counts.
    assert sum(samples[0::3]) != sum(samples[1::3])
    for (x, y), expected in (
        ((0, 0), (10311300.0, 21840840.0, 15163200.0)),
        ((408, 272), (45983700.0, 85257360.0, 52439400.0)),
        ((815, 543), (2778300.0, 3959280.0, 2357100.0)),
    ):
        offset = (y * width + x) * 3
        assert samples[offset : offset + 3] == pytest.approx(expected)
