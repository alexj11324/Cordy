import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('staging', ROOT / 'nebula-staging-start.py')
staging = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(staging)


class StagingTests(unittest.TestCase):
    def test_database_password_is_encoded(self):
        password = 'test@:/?#%'
        url = urlsplit(staging.database_url(password))
        self.assertEqual(unquote(url.password), password)
        self.assertEqual(url.hostname, 'postgres')

    def test_snapshot_is_recoverable(self):
        manifest = json.loads((ROOT / 'nebula-staging.manifest.json').read_text())
        self.assertRegex(manifest['source_sha'], r'^[a-f0-9]{40}$')
        self.assertEqual(set(manifest['images']), {'backend', 'web', 'docs', 'auth-broker'})
        for name, image in manifest['images'].items():
            self.assertRegex(image, rf'^ghcr.io/alexj11324/patchbay-{name}@sha256:[a-f0-9]{{64}}$')

    def test_readiness_rejects_unready_and_stale_builds(self):
        for status, commit in [(503, 'current'), (200, 'old'), (200, None)]:
            with self.subTest(status=status, commit=commit):
                with self.assertRaises(ValueError):
                    staging.require_ready(SimpleNamespace(status=status, headers={'X-Patchbay-Commit': commit}), 'current')
        staging.require_ready(SimpleNamespace(status=200, headers={'X-Patchbay-Commit': 'current'}), 'current')

    @patch.object(staging.time, 'sleep')
    @patch.object(staging, 'urlopen')
    def test_waits_through_migrations_then_checks_every_application(self, opened, sleep):
        ready = MagicMock()
        ready.__enter__.return_value = SimpleNamespace(status=200, headers={'X-Patchbay-Commit': 'current'})
        opened.side_effect = [OSError('migrating'), ready, ready, ready, ready]
        staging.wait_for_applications('current')
        self.assertEqual(opened.call_count, 5)
        self.assertEqual(sleep.call_count, 1)
        self.assertEqual([call.args[0].full_url for call in opened.call_args_list][-4:], [
            'http://127.0.0.1:18211/readyz', 'http://127.0.0.1:13111/login',
            'http://127.0.0.1:14001/docs', 'http://127.0.0.1:14301/readyz'])

    def test_timeout_cannot_report_success(self):
        with self.assertRaises(TimeoutError):
            staging.wait_for_applications('current', timeout=0)


if __name__ == '__main__':
    unittest.main()
