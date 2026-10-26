import importlib.util
import json
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "prepare", Path(__file__).with_name("prepare.py")
)
assert spec is not None and spec.loader is not None
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class PreparationTests(unittest.TestCase):
    def test_embeds_buffer_offsets_and_preserves_texture_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.bin").write_bytes(b"abcd")
            (root / "b.bin").write_bytes(b"efgh")
            texture = b"\x89PNG\r\n\x1a\nuntouched-texture"
            (root / "texture.png").write_bytes(texture)
            document = dict(
                asset=dict(version="2.0"),
                buffers=[
                    dict(uri="a.bin", byteLength=4),
                    dict(uri="b.bin", byteLength=4),
                ],
                bufferViews=[dict(buffer=1, byteLength=4)],
                images=[dict(uri="texture.png")],
            )
            source = root / "scene.gltf"
            source.write_text(json.dumps(document))
            packed = prepare.pack(source)
            magic, version, length = struct.unpack_from("<III", packed)
            self.assertEqual((magic, version, length), (0x46546C67, 2, len(packed)))
            size = struct.unpack_from("<I", packed, 12)[0]
            result = json.loads(packed[20 : 20 + size])
            self.assertEqual(result["bufferViews"][0]["byteOffset"], 4)
            raw = packed[28 + size :]
            view = result["bufferViews"][result["images"][0]["bufferView"]]
            self.assertEqual(
                raw[view["byteOffset"] : view["byteOffset"] + view["byteLength"]],
                texture,
            )

    def test_refuses_archive_traversal_before_extracting(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "unsafe.zip"
            with zipfile.ZipFile(archive, "w") as zipped:
                zipped.writestr("../escape", "bad")
            with self.assertRaises(ValueError):
                prepare.prepare(
                    archive,
                    root / "out",
                    scale=1.0,
                    spawn=[0, 0, 0],
                    translation=[0, 0, 0],
                    rotation=[0, 0, 0],
                )
            self.assertFalse((root / "out").exists())


if __name__ == "__main__":
    unittest.main()
