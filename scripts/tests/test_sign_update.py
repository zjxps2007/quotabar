"""Exercise the release signer using disposable repositories and throwaway keys."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class SigningTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='quotabar-sign-test-')
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.assets = self.root / 'assets'
        self.output = self.root / 'output'
        (self.repo / 'scripts').mkdir(parents=True)
        (self.repo / 'shared/assets').mkdir(parents=True)
        self.assets.mkdir()
        shutil.copy2(Path(__file__).resolve().parents[1] / 'sign-update.py', self.repo / 'scripts/sign-update.py')
        self.key = self.root / 'test-private.pem'
        self.call('openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', str(self.key))
        self.key.chmod(0o600)
        public = self.call('openssl', 'pkey', '-in', str(self.key), '-pubout').stdout
        (self.repo / 'shared/assets/UpdatePublicKey.pem').write_bytes(public)
        (self.repo / 'shared/version.json').write_text('{"version":"0.1.1","build":7}')
        self.call('git', 'init', '-q', str(self.repo))
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        commit = self.git('rev-parse', 'HEAD').stdout.decode().strip()
        files = []
        for suffix in ('macOS-arm64.zip', 'Windows-x64.zip', 'Windows-x64-Setup.exe'):
            name = 'QuotaBar-0.1.1-' + suffix
            data = ('synthetic ' + suffix).encode()
            (self.assets / name).write_bytes(data)
            files.append({'name': name, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
        self.mac = {'version': '0.1.1', 'build': 7, 'application_commit': commit, 'source_dirty': False, 'application_zip': files[0]}
        self.win = {'version': '0.1.1', 'application_commit': commit, 'installer_commit': commit, 'installer_source_dirty': False,
                    'application_zip': files[1], 'installer': files[2]}
        self.metadata()

    def tearDown(self):
        self.temp.cleanup()

    def call(self, *args, **kwargs):
        return subprocess.run(args, check=True, capture_output=True, **kwargs)

    def git(self, *args):
        return self.call('git', '-C', str(self.repo), *args)

    def metadata(self):
        (self.assets / 'macos-build-info.json').write_text(json.dumps(self.mac))
        (self.assets / 'installer-build-info.json').write_text(json.dumps(self.win))

    def sign(self, success=True):
        result = subprocess.run([sys.executable, str(self.repo / 'scripts/sign-update.py'), '--artifacts', str(self.assets),
                                 '--output', str(self.output), '--private-key', str(self.key)], capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stderr.decode())
        if not success:
            self.assertFalse((self.output / 'update-manifest.sig').exists())

    def test_exact_assets_produce_verifiable_signature(self):
        self.sign()
        self.call('openssl', 'dgst', '-sha256', '-verify', str(self.repo / 'shared/assets/UpdatePublicKey.pem'),
                  '-signature', str(self.output / 'update-manifest.sig'), str(self.output / 'update-manifest.json'))
        manifest = json.loads((self.output / 'update-manifest.json').read_text())
        self.assertEqual(len(manifest['assets']), 3)
        self.assertEqual(manifest['version'], '0.1.1')
        self.assertNotIn(b'PRIVATE KEY', (self.output / 'update-manifest.json').read_bytes())

    def test_mismatched_commit_is_rejected(self):
        self.win['installer_commit'] = '0' * 40
        self.metadata(); self.sign(False)

    def test_modified_payload_is_rejected(self):
        (self.assets / self.win['installer']['name']).write_bytes(b'tampered')
        self.sign(False)

    def test_dirty_checkout_is_rejected(self):
        (self.repo / 'shared/version.json').write_text('{"version":"0.1.1","build":8}')
        self.sign(False)

    def test_wrong_signing_key_is_rejected(self):
        self.call('openssl', 'ecparam', '-name', 'prime256v1', '-genkey', '-noout', '-out', str(self.key))
        self.sign(False)

    def test_duplicate_asset_is_rejected(self):
        (self.assets / 'duplicate').mkdir()
        shutil.copy2(self.assets / self.win['installer']['name'], self.assets / 'duplicate' / self.win['installer']['name'])
        self.sign(False)


if __name__ == '__main__':
    unittest.main()
