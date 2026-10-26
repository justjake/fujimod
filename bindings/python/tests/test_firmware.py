"""Firmware binding checks: inversion, decompression and container parsing."""

import hashlib
import struct
import unittest

from fujimod import firmware


class Firmware(unittest.TestCase):
    def test_stream_and_scan(self) -> None:
        stream = struct.pack("<5I", 16, 24, 1, 0x4000, 4) + b"\x01A\x80\x1f"
        decoded, metadata = firmware.decompress(stream)
        self.assertEqual(decoded, b"A" * 16)
        self.assertEqual(metadata["token_counts"], dict(literals=1, matches=1))
        source = firmware.invert(stream)
        self.assertEqual(
            firmware.scan(source)[0]["sha256"], hashlib.sha256(decoded).hexdigest()
        )
        image = firmware.decode(source, [("code", 0, 16, 0x1000, True, True)])
        self.assertEqual(image.source_sha256, hashlib.sha256(source).hexdigest())
        self.assertEqual(
            (
                image.units[0].name,
                image.units[0].base,
                image.units[0].bytes,
                image.units[0].executable,
            ),
            ("code", 0x1000, decoded, True),
        )
        with self.assertRaises(ValueError):
            firmware.decompress(stream[:-1])

    def test_headers_and_raw(self) -> None:
        data = struct.pack("<8I", 0, 0x1000, 32, 0x1010, 0, 0, 0, 0xAAAAFFFF)
        self.assertEqual(firmware.inner_header(data), (32, 0x1000, 32))
        digest = hashlib.sha256(data).hexdigest()
        self.assertEqual(
            firmware.raw(data, 0, 32, 0x1000, digest, 32, 32, 0x1010), data
        )
        with self.assertRaises(ValueError):
            firmware.raw(data, 0, 32, 0x1000, digest, 64, 32, 0x1010)
        outer = bytearray(0x2BC)
        struct.pack_into("<I", outer, 0, 8)
        struct.pack_into("<2I", outer, 0x204, 1, 0x23)
        signature = b"\x30\x06\x02\x01\x01\x02\x01\x02"
        header = firmware.outer_header(bytes(outer) + signature)
        self.assertEqual(
            (header["kind"], header["signed_size"], header["signature_der"]),
            (8, len(outer), signature),
        )


if __name__ == "__main__":
    unittest.main()
