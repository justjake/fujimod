"""Shared scene assets for native runs and the browser build."""
from hashlib import sha256
from pathlib import Path
import subprocess

_DEFAULT_RAF_SHA256 = '2fdbda765e9abf147d4bdfdd5a2060d2c8702d7ad53561a5bde664bc50d53cf4'
# File states already verified in this process: (size, mtime, inode).
_verified: set[tuple[int, int, int]] = set()


def _state(asset: Path) -> tuple[int, int, int] | None:
    if not asset.is_file():
        return None
    stat = asset.stat()
    return (stat.st_size, stat.st_mtime_ns, stat.st_ino)


def _matches(asset: Path) -> bool:
    state = _state(asset)
    if state is None:
        return False
    if state not in _verified:
        if sha256(asset.read_bytes()).hexdigest() != _DEFAULT_RAF_SHA256:
            return False
        _verified.add(state)
    return True


def default_scene_raf() -> Path:
    """Return the verified original RAF, materializing it when missing or stale.

    Each file state is hashed once per process."""
    root = Path(__file__).resolve().parents[4]
    asset = root / 'build/assets/default-scene.raf'
    if not _matches(asset):
        subprocess.run(['node', str(root / 'scripts/assets/default-scene.ts')],
                       cwd=root, check=True, capture_output=True, text=True)
        if not _matches(asset):
            raise ValueError('Default RAF does not match the recorded original SHA-256')
    return asset
