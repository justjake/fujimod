"""Pack an authorized Sketchfab glTF download into one portable environment.

No network access. Optionally install a reviewed, optimized runtime GLB.
The source archive is kept outside Git. Prepared runtime GLBs are LFS assets.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import struct
import tempfile
import zipfile
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parent


def safe_path(root: Path, relative: str) -> Path:
    path = (root / unquote(relative)).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Asset escapes archive: {relative}")
    return path


def pack(source: Path) -> bytes:
    document = json.loads(source.read_text())
    required = set(document.get("extensionsRequired", []))
    if required & {"KHR_draco_mesh_compression", "EXT_meshopt_compression"}:
        raise ValueError(
            "Download the uncompressed glTF version; compressed geometry needs decoding first."
        )
    data = bytearray()
    offsets = []

    def append(raw: bytes) -> int:
        data.extend(b"\0" * (-len(data) % 4))
        offset = len(data)
        data.extend(raw)
        return offset

    def read(uri: str) -> bytes:
        if uri.startswith("data:"):
            header, encoded = uri.split(",", 1)
            if not header.endswith(";base64"):
                raise ValueError("Only base64 data URIs are supported")
            return base64.b64decode(encoded, validate=True)
        if ":" in uri:
            raise ValueError("Environment assets must be local to the download")
        return safe_path(source.parent, uri).read_bytes()

    for buffer in document.get("buffers", []):
        raw = read(buffer["uri"])
        if len(raw) != buffer["byteLength"]:
            raise ValueError("glTF buffer length does not match its declaration")
        offsets.append(append(raw))
    views = document.setdefault("bufferViews", [])
    for view in views:
        view["byteOffset"] = view.get("byteOffset", 0) + offsets[view["buffer"]]
        view["buffer"] = 0
    for image in document.get("images", []):
        if "uri" not in image:
            continue
        uri = image.pop("uri")
        raw = read(uri)
        if raw.startswith(b"\x89PNG"):
            mime = "image/png"
        elif raw.startswith(b"\xff\xd8"):
            mime = "image/jpeg"
        elif raw.startswith(b"\xabKTX 20\xbb\r\n\x1a\n"):
            mime = "image/ktx2"
        else:
            raise ValueError("Expected PNG, JPEG or KTX2 texture")
        image["bufferView"] = len(views)
        image["mimeType"] = mime
        views.append(dict(buffer=0, byteOffset=append(raw), byteLength=len(raw)))
    document["buffers"] = [dict(byteLength=len(data))]
    encoded = json.dumps(document, separators=(",", ":")).encode()
    encoded += b" " * (-len(encoded) % 4)
    data.extend(b"\0" * (-len(data) % 4))
    return (
        struct.pack("<III", 0x46546C67, 2, 28 + len(encoded) + len(data))
        + struct.pack("<II", len(encoded), 0x4E4F534A)
        + encoded
        + struct.pack("<II", len(data), 0x004E4942)
        + data
    )


def prepare(
    archive: Path,
    destination: Path,
    *,
    scale: float,
    spawn: list[float],
    translation: list[float],
    rotation: list[float],
    runtime_scene: Path | None = None,
) -> dict[str, str | float | list[float] | bool]:
    if (
        not math.isfinite(scale)
        or scale <= 0
        or not all(math.isfinite(v) for v in spawn + translation + rotation)
    ):
        raise ValueError("Expected finite positions and a positive scale")
    if destination.exists():
        raise FileExistsError(
            f"Refusing to replace an existing environment: {destination}"
        )
    with tempfile.TemporaryDirectory(prefix="fujimod-gltf-") as directory:
        root = Path(directory)
        with zipfile.ZipFile(archive) as zipped:
            for item in zipped.infolist():
                safe_path(root, item.filename)
                if item.external_attr >> 16 & 0o170000 == 0o120000:
                    raise ValueError("Archive symlinks are not supported")
            zipped.extractall(root)
        sources = sorted(root.rglob("*.gltf"))
        if len(sources) != 1:
            raise ValueError("Expected one glTF scene in the Sketchfab archive")
        packed = runtime_scene.read_bytes() if runtime_scene else pack(sources[0])
        if packed[:4] != b"glTF":
            raise ValueError("Runtime scene must be a GLB")
    manifest = dict(
        id="crouch-end-station",
        title="Crouch End Station",
        author="artfletch",
        source="https://sketchfab.com/3d-models/crouch-end-station-d2651a5550f34d80b0dba6e971f3a1ae",
        license="https://creativecommons.org/licenses/by/4.0/",
        scene="environments/crouch-end-station/assets/scene.glb",
        scale=scale,
        spawn=spawn,
        translation=translation,
        rotation=rotation,
        yaw=0.0,
        unlit=True,
        sha256=hashlib.sha256(packed).hexdigest(),
        source_sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
    )
    (destination / "assets").mkdir(parents=True)
    (destination / "assets/scene.glb").write_bytes(packed)
    (destination / "environment.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Fresh environment directory; review before installing here",
    )
    parser.add_argument(
        "--runtime-scene",
        type=Path,
        help="Reviewed output of tools/prepare-photogrammetry.ts",
    )
    parser.add_argument(
        "--scale",
        type=float,
        required=True,
        help="Meters per source unit; verify from geometry",
    )
    parser.add_argument("--spawn", type=float, nargs=3, required=True)
    parser.add_argument("--translation", type=float, nargs=3, default=[0.0, 0.0, 0.0])
    parser.add_argument(
        "--rotation", type=float, nargs=3, default=[0.0, 0.0, 0.0], help="XYZ radians"
    )
    args = parser.parse_args()
    print(
        json.dumps(
            prepare(
                args.archive,
                args.output,
                scale=args.scale,
                spawn=args.spawn,
                translation=args.translation,
                rotation=args.rotation,
                runtime_scene=args.runtime_scene,
            ),
            indent=2,
        )
    )
