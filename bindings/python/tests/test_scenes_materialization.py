"""Missing and stale derived files are repaired by the TypeScript asset builder."""
from pathlib import Path
import shutil

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
