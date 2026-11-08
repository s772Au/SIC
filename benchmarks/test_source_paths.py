import json
import hashlib
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from contextlib import redirect_stdout
from source_paths import resolve_source, public_source_path, report_path
import verify_sources


class SourcePaths(unittest.TestCase):
    entry = {"id": "corpusqa-4m", "path": "benchmarks/data/corpusqa/4m_4domains.jsonl"}

    def test_private_mount_never_changes_public_identity(self):
        with tempfile.TemporaryDirectory() as folder, patch.dict(os.environ, {}, clear=True):
            root = Path(folder)
            (root / '.local').mkdir()
            private = root / 'private-data' / 'large.jsonl'
            (root / '.local/source-overrides.json').write_text(json.dumps({'sources': {'corpusqa-4m': str(private)}}))
            self.assertEqual(resolve_source(self.entry, root), private.resolve())
            self.assertEqual(public_source_path(self.entry), 'benchmarks/data/corpusqa/4m_4domains.jsonl')
            with patch.dict(os.environ, {'SIC_DATA_ROOT': str(root / 'explicit-data')}):
                self.assertEqual(resolve_source(self.entry, root), (root / 'explicit-data/corpusqa/4m_4domains.jsonl').resolve())

    def test_explicit_override_wins_and_no_implicit_mount_leaks(self):
        with tempfile.TemporaryDirectory() as folder, patch.dict(os.environ, {}, clear=True):
            root = Path(folder)
            config = root / 'mounts.json'
            config.write_text(json.dumps({'sources': {'corpusqa-4m': 'private/record.jsonl'}}))
            with patch.dict(os.environ, {'SIC_SOURCE_OVERRIDES': str(config), 'SIC_DATA_ROOT': str(root / 'data')}):
                self.assertEqual(resolve_source(self.entry, root), (root / 'private/record.jsonl').resolve())
            self.assertEqual(resolve_source(self.entry, root), (root / self.entry['path']).resolve())

    def test_public_paths_refuse_absolute_or_traversal(self):
        for value in ['/private/file', 'Z:/private/file', '../private/file', 'other/file']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                public_source_path({'path': value})

    def test_verification_output_contains_only_public_source_path(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            private = root / 'private' / 'record.jsonl'
            private.parent.mkdir()
            private.write_bytes(b'x')
            entry = dict(self.entry, bytes=1, sha256=hashlib.sha256(b'x').hexdigest())
            (root / 'sources-lock.json').write_text(json.dumps({'files': [entry]}))
            output = io.StringIO()
            with patch.object(verify_sources, 'ROOT', root), patch.object(verify_sources, 'resolve_source', return_value=private), \
                    patch('sys.argv', ['verify_sources', '--hash', '--all-locked']), redirect_stdout(output):
                verify_sources.main()
            report = json.loads(output.getvalue())
            self.assertEqual(report[0]['path'], self.entry['path'])
            self.assertTrue(report[0]['hashOk'])
            self.assertNotIn(str(private), output.getvalue())
            self.assertNotIn('resolved_path', output.getvalue())


if __name__ == '__main__':
    unittest.main()
