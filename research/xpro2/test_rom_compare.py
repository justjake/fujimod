import importlib.util
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('rom_compare',Path(__file__).with_name('compare-rom.py'))
rom=importlib.util.module_from_spec(spec);spec.loader.exec_module(rom)


class RomComparisonTests(unittest.TestCase):
    def test_stable_window_is_not_hardware_qualification(self):
        value=rom.compare(bytes(32768),bytes(32768))
        self.assertTrue(value['reads_identical'])
        self.assertFalse(value['physical_boot_qualified']);self.assertFalse(value['ready_to_flash'])
    def test_difference_is_not_reference_match(self):
        value=rom.compare(bytes(32768),bytes(32767)+b'x')
        self.assertFalse(value['reads_identical']);self.assertFalse(value['matches_xt2_reference'])
    def test_old_overread_file_is_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'window.bin';path.write_bytes(bytes(65536))
            with self.assertRaises(ValueError):rom.read_window(path)

if __name__=='__main__':unittest.main()
