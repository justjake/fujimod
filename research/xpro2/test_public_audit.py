"""Publication checks use synthetic paths and tokens, never real credentials."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('publication_audit',Path(__file__).with_name('audit-public.py'))
audit=importlib.util.module_from_spec(spec);spec.loader.exec_module(audit)


class PublicationAuditTests(unittest.TestCase):
    def test_relative_source_path_is_not_a_personal_directory(self):
        with tempfile.TemporaryDirectory() as folder,patch.object(audit,'HERE',Path(folder)):
            (Path(folder)/'metadata.json').write_text('{"path":"apps/web/src/home/controller.ts"}')
            self.assertIn('metadata.json',audit.inspect())
    def test_personal_absolute_path_rejected(self):
        with tempfile.TemporaryDirectory() as folder,patch.object(audit,'HERE',Path(folder)):
            private='/'.join(['','home','example-user','capture'])
            (Path(folder)/'notes.md').write_text('File: '+private)
            with self.assertRaisesRegex(ValueError,'personal directory'):audit.inspect()
    def test_token_and_binary_inputs_rejected(self):
        with tempfile.TemporaryDirectory() as folder,patch.object(audit,'HERE',Path(folder)):
            p=Path(folder)/'notes.md';p.write_text('ghp_'+'a'*40)
            with self.assertRaisesRegex(ValueError,'token'):audit.inspect()
            p.unlink();(Path(folder)/'capture.bin').write_bytes(b'\0'*4)
            with self.assertRaisesRegex(ValueError,'binary'):audit.inspect()
    def test_large_json_blob_rejected(self):
        import json
        with tempfile.TemporaryDirectory() as folder,patch.object(audit,'HERE',Path(folder)):
            (Path(folder)/'metadata.json').write_text(json.dumps({'payload':list(range(129))}))
            with self.assertRaisesRegex(ValueError,'large binary'):audit.inspect()

if __name__=='__main__':unittest.main()
