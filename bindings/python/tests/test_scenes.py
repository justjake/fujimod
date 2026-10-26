"""The default asset is shared with the web build and retains sensor radiance."""
from hashlib import sha256
import math
import struct

from fujimod import scenes
from fujimod._native import emulator


def test_default_scene_is_verified_and_reused():
    asset = scenes.default_scene_raf()
    assert sha256(asset.read_bytes()).hexdigest() == scenes._DEFAULT_RAF_SHA256
    modified = asset.stat().st_mtime_ns
    assert scenes.default_scene_raf() == asset
    assert asset.stat().st_mtime_ns == modified


def test_default_raf_retains_linear_color_in_its_visible_sensor_crop():
    width, height, data = emulator.raf_radiance_rgb(scenes.default_scene_raf().read_bytes())
    assert (width, height) == (816, 544)
    assert sha256(data).hexdigest() == "2eef6e993795ee33fa7ad33c577fca92a1a690f5cbc12b3a9e39673d8da2af70"
    assert len(data) == width * height * 3 * 4
    samples = struct.unpack(f'<{width*height*3}f', data)
    assert all(math.isfinite(value) and value >= 0 for value in samples)
    assert max(samples) > 16383  # Exposure-normalized radiance, not clipped ADC counts.
    assert sum(samples[0::3]) != sum(samples[1::3])
