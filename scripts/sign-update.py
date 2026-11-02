#!/usr/bin/env python3
"""Sign the exact, verified release assets for both native updaters."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--private-key', type=Path, help='Otherwise use QUOTABAR_UPDATE_SIGNING_KEY from the environment.')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    version = json.loads((repo / 'shared/version.json').read_text())
    require(re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version['version']), 'Invalid stable version.')
    require(type(version['build']) is int and version['build'] > 0, 'Invalid build number.')
    commit = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()
    require(not subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain', '--untracked-files=normal'], text=True).strip(),
            'Release signing requires a clean checkout.')

    def find(name):
        files = list(args.artifacts.rglob(name))
        if len(files) != 1 or not files[0].is_file() or files[0].is_symlink():
            raise ValueError('Expected one regular release file: ' + name)
        return files[0]

    mac = json.loads(find('macos-build-info.json').read_text(encoding='utf-8-sig'))
    win = json.loads(find('installer-build-info.json').read_text(encoding='utf-8-sig'))
    require(mac['application_commit'] == win['application_commit'] == win['installer_commit'] == commit, 'Artifact source commits differ.')
    require(mac['source_dirty'] is False and win['installer_source_dirty'] is False, 'Artifact sources were modified.')
    require(mac['version'] == win['version'] == version['version'] and mac['build'] == version['build'], 'Artifact versions differ.')
    expected = [mac['application_zip'], win['application_zip'], win['installer']]
    names = {f'QuotaBar-{version["version"]}-{suffix}' for suffix in ('macOS-arm64.zip', 'Windows-x64.zip', 'Windows-x64-Setup.exe')}
    require(names == {asset['name'] for asset in expected}, 'Missing or unexpected platform assets.')
    assets = []
    for asset in sorted(expected, key=lambda item: item['name']):
        path = find(asset['name'])
        digest = sha256(path)
        require(digest == asset['sha256'], 'Artifact digest differs: ' + path.name)
        require(0 < path.stat().st_size <= 512 * 1024 * 1024, 'Invalid artifact size: ' + path.name)
        if 'bytes' in asset:
            require(path.stat().st_size == asset['bytes'], 'Artifact size differs: ' + path.name)
        assets.append({'name': path.name, 'bytes': path.stat().st_size, 'sha256': digest})
    manifest = {'schemaVersion': 1, **version, 'assets': assets}
    args.output.mkdir(parents=True, exist_ok=True)
    manifest_path = args.output / 'update-manifest.json'
    signature_path = args.output / 'update-manifest.sig'
    payload = (json.dumps(manifest, sort_keys=True, separators=(',', ':')) + '\n').encode()
    with tempfile.TemporaryDirectory(prefix='quotabar-sign-') as temporary:
        key = args.private_key
        if key is None:
            value = os.environ.get('QUOTABAR_UPDATE_SIGNING_KEY')
            if not value:
                raise ValueError('Update signing key is not configured.')
            key = Path(temporary) / 'private.pem'
            key.touch(mode=0o600)
            key.write_text(value)
        public = subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout'], check=True, capture_output=True).stdout
        if public.strip() != (repo / 'shared/assets/UpdatePublicKey.pem').read_bytes().strip():
            raise ValueError('Signing key does not match the public key embedded in the apps.')
        pending = Path(temporary) / 'manifest.json'
        pending.write_bytes(payload)
        signature = Path(temporary) / 'manifest.sig'
        subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(key), '-out', str(signature), str(pending)], check=True, capture_output=True)
        subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(repo / 'shared/assets/UpdatePublicKey.pem'),
                        '-signature', str(signature), str(pending)], check=True, capture_output=True)
        manifest_path.write_bytes(payload)
        signature_path.write_bytes(signature.read_bytes())
    print('Signed update manifest:', version['version'], 'build', version['build'], 'source', commit)
    print('Manifest SHA256:', sha256(manifest_path))


if __name__ == '__main__':
    main()
