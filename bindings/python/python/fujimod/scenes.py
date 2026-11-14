"""Shared scene assets for native runs and the browser build."""
from pathlib import Path
import subprocess

_DEFAULT_RAF_SIZE = 33_786_368
# Last accepted state per materialized file: (size, mtime, inode).
_materialized: dict[Path, tuple[int, int, int]] = {}


def _state(asset: Path) -> tuple[int, int, int] | None:
    if not asset.is_file():
        return None
    stat = asset.stat()
    return (stat.st_size, stat.st_mtime_ns, stat.st_ino)


def default_scene_raf() -> Path:
    """Return the shared RAF, materializing missing, resized or changed files."""
    root = Path(__file__).resolve().parents[4]
    asset = root / 'build/assets/default-scene.raf'
    state = _state(asset)
    if (
        state is None
        or state[0] != _DEFAULT_RAF_SIZE
        or state != _materialized.get(asset, state)
    ):
        made = subprocess.run(['node', str(root / 'scripts/assets/default-scene.ts')],
                              cwd=root, capture_output=True, text=True)
        if made.returncode:
            raise RuntimeError('Default RAF preparation failed:\n' + made.stderr + made.stdout)
        state = _state(asset)
        if state is None or state[0] != _DEFAULT_RAF_SIZE:
            raise ValueError('Default RAF is missing or has an unexpected byte count')
    _materialized[asset] = state
    return asset
