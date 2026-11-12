"""The role Node payload packages carry the same executables on every
platform: package-role-node.sh (Linux, from the images' staged executables)
and package-role-node-darwin.sh (macOS on Apple silicon, built on the Mac)
name the same files for each role. The packages say which variants they
carry, and a role image installs its own architecture's Linux variant from a
package that carries several (the catalog's: darwin-arm64 sorts first)."""
import json
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

DEPLOY = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS = DEPLOY / 'scripts'
MODULES = DEPLOY.parent / 'modules'


def executables(script):
    text = (SCRIPTS / script).read_text()
    found = {}
    for role in ('maker', 'taker'):
        listed = re.search(rf'^  {role}\) bins=\(([^)]*)\) ;;$', text, re.M)
        common = re.search(r'^bins\+=\(([^)]*)\)$', text, re.M)
        found[role] = sorted(listed.group(1).split() + common.group(1).split())
    return found


class RolePackages(unittest.TestCase):
    def test_linux_and_darwin_packages_carry_the_same_executables(self):
        linux = executables('package-role-node.sh')
        darwin = executables('package-role-node-darwin.sh')
        self.assertEqual(linux, darwin)
        self.assertIn('lez-maker-node', linux['maker'])
        self.assertNotIn('lez-taker-node', linux['maker'])
        self.assertIn('lez-v02-bridge-poc', linux['taker'])

    def test_metadata_names_every_variant_it_is_built_for(self):
        for role in ('maker', 'taker'):
            description = json.loads((MODULES / f'lez-{role}-node/metadata.json').read_text())['description']
            self.assertNotIn('Linux only', description, role)
            for variant in ('linux-arm64', 'linux-amd64', 'darwin-arm64'):
                self.assertIn(variant, description, (role, variant))


def variant_selection(role):
    """The shell of the role Dockerfile's install step that picks the variant."""
    text = (DEPLOY / f'images/{role}-node/Dockerfile').read_text()
    run = text.split(f'COPY --chmod=0444 lez_{role}_node.lgx', 1)[1].split('\nCOPY', 1)[0]
    start = run.index('case "$(uname -m)"')
    end = run.index('mkdir -p "$module"')
    return run[start:end]


@unittest.skipUnless(shutil.which('jq') and shutil.which('bash'), 'needs jq and bash')
class RoleImagesPickTheirVariant(unittest.TestCase):
    MERGED = {'name': 'lez_maker_node', 'main': {
        'darwin-arm64': 'p.dylib', 'linux-amd64': 'p.so', 'linux-arm64': 'p.so'}}

    def pick(self, role, machine, manifest):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            (tmp / 'bin').mkdir()
            (tmp / 'bin/uname').write_text(f'#!/bin/sh\necho {machine}\n')
            (tmp / 'bin/uname').chmod(0o755)
            (tmp / 'manifest.json').write_text(json.dumps(manifest))
            script = (f'set -eu; unpacked={tmp}\n' + variant_selection(role)
                      + '\nprintf %s "$variant"\n')
            env = {'PATH': f'{tmp}/bin:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin'}
            return subprocess.run(['bash', '-c', script], env=env, capture_output=True, text=True)

    def test_neither_image_takes_the_first_variant(self):
        for role in ('maker', 'taker'):
            text = (DEPLOY / f'images/{role}-node/Dockerfile').read_text()
            self.assertNotIn('keys[0]', text, role)

    def test_each_architecture_gets_its_linux_variant_from_a_merged_package(self):
        for role in ('maker', 'taker'):
            for machine, variant in (('aarch64', 'linux-arm64'), ('x86_64', 'linux-amd64')):
                done = self.pick(role, machine, self.MERGED)
                self.assertEqual(done.returncode, 0, (role, machine, done.stderr))
                self.assertEqual(done.stdout, variant, (role, machine))

    def test_a_package_without_the_architecture_is_refused(self):
        for role in ('maker', 'taker'):
            done = self.pick(role, 'x86_64', {'main': {'darwin-arm64': 'p.dylib', 'linux-arm64': 'p.so'}})
            self.assertNotEqual(done.returncode, 0, role)
            self.assertIn('no linux-amd64 variant', done.stderr, role)
            done = self.pick(role, 'riscv64', self.MERGED)
            self.assertNotEqual(done.returncode, 0, role)


if __name__ == '__main__':
    unittest.main()
