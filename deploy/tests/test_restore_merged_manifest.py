"""restore-merged-manifest.py gives a merged package back the manifest fields
`lgx merge` drops (display_name), and refuses inputs that disagree."""
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'scripts/restore-merged-manifest.py'
BASE = {'name': 'lez_core', 'version': '0.4.1-sign.4', 'display_name': 'LEZ Core Module',
        'type': 'core', 'manifestVersion': '0.3.0', 'dependencies': []}


def lgx(path, manifest, variants):
    with tarfile.open(path, 'w:gz') as t:
        for name, data in [('manifest.json', json.dumps(manifest).encode())] + [
                (f'variants/{v}/lib', v.encode()) for v in variants]:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))


def manifest_of(path):
    with tarfile.open(path, 'r:gz') as t:
        return json.loads(t.extractfile('manifest.json').read())


class RestoreMergedManifest(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp())
        self.linux = self.dir / 'linux.lgx'
        self.darwin = self.dir / 'darwin.lgx'
        lgx(self.linux, dict(BASE, main={'linux-arm64': 'p.so'}, hashes={'root': 'l'}), ['linux-arm64'])
        lgx(self.darwin, dict(BASE, main={'darwin-arm64': 'p.dylib'}, hashes={'root': 'd'}), ['darwin-arm64'])
        merged = {k: v for k, v in BASE.items() if k != 'display_name'}  # what lgx merge writes
        self.merged = self.dir / 'merged.lgx'
        self.main = {'darwin-arm64': 'p.dylib', 'linux-arm64': 'p.so'}
        lgx(self.merged, dict(merged, main=self.main, hashes={'root': 'm'}), ['darwin-arm64', 'linux-arm64'])

    def run_it(self, *inputs):
        return subprocess.run([sys.executable, str(SCRIPT), str(self.merged), *map(str, inputs)],
                              capture_output=True, text=True)

    def test_the_dropped_fields_come_back_with_the_merged_variants(self):
        done = self.run_it(self.linux, self.darwin)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn('restored display_name', done.stdout)
        got = manifest_of(self.merged)
        self.assertEqual(got['display_name'], 'LEZ Core Module')
        self.assertEqual(got['main'], self.main)
        self.assertEqual(got['hashes'], {'root': 'm'})
        with tarfile.open(self.merged, 'r:gz') as t:
            self.assertEqual(sorted(t.getnames()),
                             ['manifest.json', 'variants/darwin-arm64/lib', 'variants/linux-arm64/lib'])

    def test_variants_that_disagree_are_refused(self):
        lgx(self.darwin, dict(BASE, description='other', main={'darwin-arm64': 'p.dylib'}, hashes={}), ['darwin-arm64'])
        done = self.run_it(self.linux, self.darwin)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('differ in description', done.stderr)

    def test_inputs_must_carry_exactly_the_merged_variants(self):
        done = self.run_it(self.linux)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('its inputs', done.stderr)


if __name__ == '__main__':
    unittest.main()
