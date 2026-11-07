"""Missing and stale derived files are repaired by the TypeScript asset builder."""
from pathlib import Path
import os
import shutil

import pytest

from fujimod import scenes


def test_default_scene_repairs_missing_and_stale_output(tmp_path, monkeypatch):
    root = Path(scenes.__file__).resolve().parents[4]
    script = tmp_path/'scripts/assets/default-scene.ts'
    script.parent.mkdir(parents=True)
    shutil.copyfile(root/'scripts/assets/default-scene.ts', script)
    original = tmp_path/'apps/web/src/engine/assets/default-scene.raf.zst'
    original.parent.mkdir(parents=True)
    original.symlink_to(root/'apps/web/src/engine/assets/default-scene.raf.zst')
    monkeypatch.setattr(scenes, '__file__', str(tmp_path/'bindings/python/python/fujimod/scenes.py'))
    asset = scenes.default_scene_raf()
    assert asset == tmp_path/'build/assets/default-scene.raf'
    expected = asset.read_bytes()
    asset.write_bytes(b'stale')
    assert scenes.default_scene_raf().read_bytes() == expected
    # A changed file state also sends same-size rewrites through the builder.
    modified = asset.stat().st_mtime_ns
    with asset.open('r+b') as output:
        output.write(b'stale')
    os.utime(asset, ns=(modified + 1, modified + 1))
    assert scenes.default_scene_raf().read_bytes() == expected


def scene_path(tmp_path, monkeypatch):
    monkeypatch.setattr(scenes, '__file__', str(tmp_path/'bindings/python/python/fujimod/scenes.py'))
    monkeypatch.setattr(scenes, '_materialized', {})
    asset = tmp_path/'build/assets/default-scene.raf'
    asset.parent.mkdir(parents=True)
    return asset


def test_existing_scene_is_reused_without_reading_its_bytes(tmp_path, monkeypatch):
    asset = scene_path(tmp_path, monkeypatch)
    # Sparse output exercises the real byte-count check without decoding a RAF.
    with asset.open('wb') as output:
        output.truncate(scenes._DEFAULT_RAF_SIZE)

    def unexpected(*args, **kwargs):
        pytest.fail('Loading an unchanged scene must not read or regenerate it')

    monkeypatch.setattr(Path, 'read_bytes', unexpected)
    monkeypatch.setattr(scenes.subprocess, 'run', unexpected)
    assert scenes.default_scene_raf() == asset
    assert scenes.default_scene_raf() == asset


@pytest.mark.parametrize('output', [None, b'short'])
def test_materialization_requires_an_existing_output_of_the_expected_size(
    tmp_path, monkeypatch, output
):
    asset = scene_path(tmp_path, monkeypatch)

    def materialize(*args, **kwargs):
        if output is not None:
            asset.write_bytes(output)

    monkeypatch.setattr(scenes.subprocess, 'run', materialize)
    with pytest.raises(ValueError, match='missing or.*byte count'):
        scenes.default_scene_raf()


def test_replacing_the_scene_inode_requires_materialization(tmp_path, monkeypatch):
    asset = scene_path(tmp_path, monkeypatch)
    monkeypatch.setattr(scenes, '_DEFAULT_RAF_SIZE', 4)
    asset.write_bytes(b'good')
    assert scenes.default_scene_raf() == asset
    modified = asset.stat().st_mtime_ns
    replacement = asset.with_suffix('.new')
    replacement.write_bytes(b'bad!')
    os.utime(replacement, ns=(modified, modified))
    replacement.replace(asset)
    calls = []

    def materialize(*args, **kwargs):
        calls.append(args)
        asset.write_bytes(b'good')

    monkeypatch.setattr(scenes.subprocess, 'run', materialize)
    assert scenes.default_scene_raf().read_bytes() == b'good'
    assert len(calls) == 1
    assert scenes.default_scene_raf() == asset
    assert len(calls) == 1
