"""The Monero release pins and their verification, for both Linux architectures.

tests/e2e/monero/provenance.env pins one release archive per architecture,
linux-x64 (x86_64) and linux-armv8 (aarch64); scripts/verify-monero-release.sh
picks the one of MONERO_ARCH and checks it as strictly for either: the signed
hash manifest against the pinned signer, the archive's sha256 and size, its
members and the binaries' --version. The pins are checked against the
retained signed manifest and the image's labels; the verifier runs end to end
on a release this test signs itself (its own key, its own manifest, archives
whose "binaries" print the release's version), offline: the source tag is read
from a stub git. That run needs GNU coreutils, gpg, ripgrep, jq and bzip2, and
is skipped where they are missing (macOS's BSD stat).
"""
import json
import os
import pathlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import hashlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
PROVENANCE = ROOT / 'tests/e2e/monero/provenance.env'
VERIFY = ROOT / 'scripts/verify-monero-release.sh'
DOCKERFILE = (ROOT / 'tests/e2e/monero/Dockerfile').read_text()
ARCHES = {'X86_64': ('x86_64', 'amd64', 'linux-x64'), 'AARCH64': ('aarch64', 'arm64', 'linux-armv8')}


def pins():
    values = {}
    for line in PROVENANCE.read_text().splitlines():
        if line and not line.startswith('#'):
            key, _, value = line.partition('=')
            values[key] = value
    return values


class MoneroReleasePins(unittest.TestCase):
    def test_each_architecture_has_its_archive_bound_by_the_signed_manifest(self):
        p = pins()
        manifest = (ROOT / p['MONERO_HASHES_SNAPSHOT']).read_text()
        self.assertEqual(hashlib.sha256(manifest.encode()).hexdigest(), p['MONERO_HASHES_SHA256'])
        for suffix, (uname, _, platform) in ARCHES.items():
            name = p[f'MONERO_ARCHIVE_NAME_{suffix}']
            self.assertEqual(name, f"monero-{platform}-v{p['MONERO_VERSION']}.tar.bz2")
            self.assertEqual(p[f'MONERO_ARCHIVE_URL_{suffix}'], f'https://downloads.getmonero.org/cli/{name}')
            self.assertRegex(p[f'MONERO_ARCHIVE_SHA256_{suffix}'], r'^[0-9a-f]{64}$')
            self.assertIn(f"\n{p[f'MONERO_ARCHIVE_SHA256_{suffix}']}  {name}\n", manifest)
            self.assertGreater(int(p[f'MONERO_ARCHIVE_SIZE_{suffix}']), 50_000_000)
            self.assertEqual(p[f'MONERO_ARCHIVE_ROOT_{suffix}'], f"monero-{uname}-linux-gnu-v{p['MONERO_VERSION']}")
        # no architecture-less archive pin is left for a consumer to take as the only one
        self.assertNotIn('MONERO_ARCHIVE_NAME', p)

    def test_the_image_labels_both_archives_and_checks_its_binaries(self):
        p = pins()
        for suffix, (_, docker_arch, _) in ARCHES.items():
            self.assertIn(f'release-archive-linux-{docker_arch}="{p[f"MONERO_ARCHIVE_NAME_{suffix}"]}"', DOCKERFILE)
            self.assertIn(f'release-archive-sha256-linux-{docker_arch}="{p[f"MONERO_ARCHIVE_SHA256_{suffix}"]}"', DOCKERFILE)
        # the ELF machine of each architecture (EM_X86_64 0x3e, EM_AARCH64 0xb7)
        self.assertIn('amd64) want=3e00', DOCKERFILE)
        self.assertIn('arm64) want=b700', DOCKERFILE)
        # the base the image runs on is the one the contract pins, and no FROM
        # names a build argument (up.sh and from-scratch.sh pull every FROM)
        self.assertIn(f"\nFROM {p['MONERO_RUNTIME_BASE']}\n", DOCKERFILE)
        self.assertNotRegex(DOCKERFILE, r'(?m)^FROM \S*\$')

    def test_staging_takes_the_hosts_architecture_and_refuses_none(self):
        stage = (ROOT / 'deploy/scripts/stage-assets.sh').read_text()
        self.assertNotIn('linux-x64 only', stage)
        self.assertIn('-e MONERO_ARCH="$monero_arch"', stage)
        # Compose's bake builder reads no Dockerfile outside deploy/: the
        # image's is copied into its context
        compose = (ROOT / 'deploy/compose.yaml').read_text()
        self.assertIn('    build: ./images/monerod\n', compose)
        self.assertIn('install -m 0644 ../tests/e2e/monero/Dockerfile images/monerod/Dockerfile', stage)
        self.assertIn('install -m 0644 ../tests/e2e/monero/Dockerfile images/monerod/Dockerfile',
                      (ROOT / 'deploy/scripts/up.sh').read_text())


def tool_missing():
    for tool in ('gpg', 'gpgconf', 'rg', 'jq', 'sha256sum', 'bzip2', 'tar', 'git'):
        if shutil.which(tool) is None:
            return tool
    if subprocess.run(['stat', '-c', '%s', '/'], capture_output=True).returncode != 0:
        return 'GNU stat'
    return None


VERSION_LINE = "Monero 'Fluorine Fermi' (v0.18.5.1-release)"


@unittest.skipIf(tool_missing(), f'the verifier needs {tool_missing()}')
class VerifierEndToEnd(unittest.TestCase):
    """verify-monero-release.sh on a self-signed release of both architectures."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = pathlib.Path(tempfile.mkdtemp())
        home = cls.tmp / 'gnupg'
        home.mkdir(mode=0o700)
        gpg = ['gpg', '--homedir', str(home), '--batch']
        subprocess.run(gpg + ['--passphrase', '', '--quick-gen-key', 'test-signer <signer@example.invalid>',
                              'ed25519', 'sign', 'never'], check=True, capture_output=True)
        listing = subprocess.run(gpg + ['--with-colons', '--list-keys'], check=True, capture_output=True, text=True).stdout
        cls.fingerprint = next(line.split(':')[9] for line in listing.splitlines() if line.startswith('fpr:'))
        cls.key = cls.tmp / 'signer.asc'
        cls.key.write_bytes(subprocess.run(gpg + ['--armor', '--export', cls.fingerprint],
                                           check=True, capture_output=True).stdout)
        cls.archives = {}
        lines = []
        for suffix, (uname, _, platform) in ARCHES.items():
            name = f'monero-{platform}-v0.18.5.1.tar.bz2'
            root = f'monero-{uname}-linux-gnu-v0.18.5.1'
            build = cls.tmp / 'build' / root
            build.mkdir(parents=True)
            for binary in ('monerod', 'monero-wallet-rpc'):
                (build / binary).write_text(f'#!/bin/sh\necho "{VERSION_LINE}"\necho "{uname}"\n')
                (build / binary).chmod(0o755)
            (build / 'LICENSE').write_text('test\n')
            archive = cls.tmp / name
            with tarfile.open(archive, 'w:bz2') as tar:
                tar.add(build, arcname=root)
            data = archive.read_bytes()
            cls.archives[suffix] = (archive, hashlib.sha256(data).hexdigest(), len(data), name, root)
            lines.append(f'{cls.archives[suffix][1]}  {name}')
        manifest = cls.tmp / 'hashes.txt'
        manifest.write_text('## SHA256 hashes\n' + '\n'.join(lines) + '\n')
        subprocess.run(gpg + ['--clearsign', '--output', str(cls.tmp / 'hashes.signed'), str(manifest)],
                       check=True, capture_output=True)
        cls.hashes = cls.tmp / 'hashes.signed'
        # the source tag, offline: a git that answers only this ls-remote
        cls.bin = cls.tmp / 'bin'
        cls.bin.mkdir()
        (cls.bin / 'git').write_text('#!/bin/sh\n[ "$1" = ls-remote ] || exit 2\n'
                                     'printf "%s\\trefs/tags/v0.18.5.1\\n%s\\trefs/tags/v0.18.5.1^{}\\n" '
                                     f'{"a" * 40} {"b" * 40}\n')
        (cls.bin / 'git').chmod(0o755)

    @classmethod
    def tearDownClass(cls):
        subprocess.run(['gpgconf', '--homedir', str(cls.tmp / 'gnupg'), '--kill', 'gpg-agent'], capture_output=True)
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def contract(self, run, **override):
        sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
        values = {
            'MONERO_VERSION': '0.18.5.1', 'MONERO_TAG': 'v0.18.5.1',
            'MONERO_SOURCE_URL': 'https://github.com/monero-project/monero.git',
            'MONERO_SOURCE_TAG_OBJECT': 'a' * 40, 'MONERO_SOURCE_COMMIT': 'b' * 40,
            'MONERO_HASHES_URL': 'https://www.getmonero.org/downloads/hashes.txt',
            'MONERO_HASHES_SHA256': sha(self.hashes), 'MONERO_HASHES_SNAPSHOT': str(self.hashes),
            'MONERO_SIGNING_KEY_URL': 'https://example.invalid/signer.asc',
            'MONERO_SIGNING_KEY_SHA256': sha(self.key), 'MONERO_SIGNING_KEY_SNAPSHOT': str(self.key),
            'MONERO_SIGNER_FINGERPRINT': self.fingerprint,
        }
        for suffix, (_, digest, size, name, root) in self.archives.items():
            values.update({f'MONERO_ARCHIVE_NAME_{suffix}': name,
                           f'MONERO_ARCHIVE_URL_{suffix}': f'https://downloads.getmonero.org/cli/{name}',
                           f'MONERO_ARCHIVE_SHA256_{suffix}': digest,
                           f'MONERO_ARCHIVE_SIZE_{suffix}': str(size),
                           f'MONERO_ARCHIVE_ROOT_{suffix}': root})
        values.update(override)
        path = run / 'provenance.env'
        path.write_text(''.join(f'{key}={value}\n' for key, value in values.items()))
        return path

    def verify(self, arch, archive=None, **override):
        run = pathlib.Path(tempfile.mkdtemp(dir=self.tmp))
        suffix = 'AARCH64' if arch in ('aarch64', 'arm64') else 'X86_64'
        archive = archive or self.archives.get(suffix, self.archives['X86_64'])[0]
        env = {**os.environ, 'PATH': f'{self.bin}:{os.environ["PATH"]}', 'TMPDIR': str(run),
               'MONERO_ARCH': arch, 'MONERO_PROVENANCE_FILE': str(self.contract(run, **override)),
               'MONERO_ARCHIVE_PATH': str(archive), 'MONERO_CACHE_DIR': str(run / 'cache'),
               'MONERO_BUILD_CONTEXT': str(run / 'context'), 'MONERO_PROVENANCE_EVIDENCE': str(run / 'evidence/provenance.json')}
        result = subprocess.run(['bash', str(VERIFY)], env=env, capture_output=True, text=True,
                                stdin=subprocess.DEVNULL, timeout=120)
        return result, run

    def test_each_architecture_verifies_its_own_archive(self):
        for arch, suffix, also in (('x86_64', 'X86_64', 'amd64'), ('aarch64', 'AARCH64', 'arm64')):
            for given in (arch, also):
                with self.subTest(arch=given):
                    result, run = self.verify(given)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    evidence = json.loads((run / 'evidence/provenance.json').read_text())
                    self.assertEqual(evidence['consumed_archive']['architecture'], arch)
                    self.assertEqual(evidence['consumed_archive']['name'], self.archives[suffix][3])
                    self.assertEqual(evidence['consumed_archive']['size_bytes'], self.archives[suffix][2])
                    self.assertEqual(evidence['signed_manifest']['signer_primary_fingerprint'], self.fingerprint)
                    self.assertEqual(sorted(p.name for p in (run / 'context/bin').iterdir()), ['monero-wallet-rpc', 'monerod'])
                    self.assertIn(arch, subprocess.run([str(run / 'context/bin/monerod')], capture_output=True, text=True).stdout)

    def test_an_architecture_without_pins_is_refused(self):
        result, _ = self.verify('riscv64')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('no pinned Monero release archive for architecture riscv64', result.stderr)
        result, _ = self.verify('aarch64', MONERO_ARCHIVE_ROOT_AARCH64='')
        self.assertIn('missing MONERO_ARCHIVE_ROOT_AARCH64', result.stderr)

    def test_the_other_architectures_archive_is_refused(self):
        result, _ = self.verify('aarch64', archive=self.archives['X86_64'][0])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum mismatch', result.stderr)

    def test_each_architecture_is_held_to_the_manifest_size_and_members(self):
        x64 = self.archives['X86_64']
        # pins that the signed manifest does not bind, though the archive matches them
        result, _ = self.verify('aarch64', archive=x64[0], MONERO_ARCHIVE_SHA256_AARCH64=x64[1],
                                MONERO_ARCHIVE_SIZE_AARCH64=str(x64[2]))
        self.assertIn('does not bind the expected archive', result.stderr)
        result, _ = self.verify('aarch64', MONERO_ARCHIVE_SIZE_AARCH64='1')
        self.assertIn('archive size mismatch', result.stderr)
        result, _ = self.verify('aarch64', MONERO_ARCHIVE_NAME_AARCH64=x64[3])
        self.assertIn('disagree on its name, URL or root', result.stderr)
        result, _ = self.verify('x86_64', MONERO_SIGNER_FINGERPRINT='0' * 40)
        self.assertIn('fingerprint mismatch', result.stderr)


if __name__ == '__main__':
    unittest.main()
