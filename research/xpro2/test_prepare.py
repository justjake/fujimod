"""Synthetic tests; no camera or firmware input is required."""
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import prepare


class PreparationTests(unittest.TestCase):
    def test_method4_literals_and_overlapping_matches(self):
        body=b'\x03abc\x80\x36'
        raw=struct.pack('<5I',9,len(body)+20,1,0x4000,4)+body
        self.assertEqual(prepare.method4(raw,0),b'abcabcabc')
    def test_method4_rejects_zero_distance_truncation_and_overflow(self):
        for body in [b'\x00',b'\x02a',b'\x80',b'\x80\x01']:
            raw=struct.pack('<5I',3,len(body)+20,1,0x4000,4)+body
            with self.subTest(body=body),self.assertRaises(ValueError):prepare.method4(raw,0)
    def test_data_recipe_cannot_execute_or_read_arbitrary_paths(self):
        for text in ['import os','.incbin "/etc/passwd", 0x0, 0x1','.org 0x0\n.byte 0x0g']:
            with self.subTest(text=text),self.assertRaises(ValueError):prepare.apply_delta(b'\0'*4,b'abc',text)
    def test_delta_literal_reference_and_final_identity(self):
        text='.org 0x0\n.byte 0x61\n.incbin "mod-reference", 0x1, 0x2'
        with patch.object(prepare,'SD_SHA',hashlib.sha256(b'abc\0').hexdigest()):
            self.assertEqual(prepare.apply_delta(b'\0'*4,b'abc',text),b'abc\0')
    def test_delta_overlap_bounds_and_wrong_output_fail(self):
        for text in ['.org 0x0\n.byte 0x61\n.org 0x0\n.byte 0x62','.org 0x4\n.byte 0x61','.org 0x0\n.incbin "mod-reference", 0x0, 0x4','.org 0x0\n.byte 0x61']:
            with self.subTest(text=text),self.assertRaises(ValueError):prepare.apply_delta(b'\0'*4,b'abc',text)
    def test_input_hashes_fail_before_parsing(self):
        with self.assertRaisesRegex(ValueError,'DAT hash'):prepare.stock_image(b'wrong',{})
        with self.assertRaisesRegex(ValueError,'patch hash'):prepare.mod_reference(b'wrong')
    def test_output_directory_never_overwritten(self):
        with tempfile.TemporaryDirectory() as folder:
            missing=Path(folder)/'missing'
            with self.assertRaisesRegex(ValueError,'already exists'):prepare.prepare(missing,missing,missing,Path(folder))
    def test_rom_rejects_overread_without_physical_io(self):
        with tempfile.TemporaryDirectory() as folder:
            f=Path(folder)/'rom.bin';f.write_bytes(b'x'*65536)
            with self.assertRaisesRegex(ValueError,'32 KiB'):prepare.read_rom(f)

if __name__=='__main__':unittest.main()
