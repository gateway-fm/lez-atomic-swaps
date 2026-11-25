"""The release set's dependency requirements admit only the packages it was
built and tested with, and each dependency is declared once.

logosctl 0.3.x resolves every entry of a manifest's `dependencies` on its own
and prefers the highest version that matches. A range on a patched upstream
module (`~0.4.1-sign.4`) also matches the Logos catalog's official lez_core
0.4.1 to 0.4.3, and a bare name next to a pinned entry pulls in the newest
official release as well (lez_core 0.5.1). pin-package-dependencies.sh is run
here on a set of small packages laid out as the module builder writes them
(bare dependency names from metadata.json) and on the set as v0.3.0 pinned it
(each name twice); stage-basecamp-package.sh then installs the bare names
Basecamp 0.2.0 reads. The catalog's set carries the Chat module of
package-chat-module.sh instead of Logos's release (issue #147): a pre-release
the desks require exactly, whose Delivery requirement is the desks' own.
Nothing here needs Docker or the network; the staging test needs jq.
"""
import hashlib
import io
import json
import pathlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
PIN = ROOT / 'deploy/scripts/pin-package-dependencies.sh'
STAGE = ROOT / 'deploy/scripts/stage-basecamp-package.sh'
CHAT = ROOT / 'deploy/scripts/package-chat-module.sh'

OURS = {
    'lez_atomic_swap_maker': 'apps/basecamp/maker/metadata.json',
    'lez_atomic_swap_taker': 'apps/basecamp/taker/metadata.json',
    'lez_network': 'apps/basecamp/network/metadata.json',
    'lez_zcash_node': 'apps/basecamp/zcash/metadata.json',
    'lez_swap_node': 'modules/lez-swap-node/metadata.json',
    'lez_maker_node': 'modules/lez-maker-node/metadata.json',
    'lez_taker_node': 'modules/lez-taker-node/metadata.json',
}
# Patched upstream modules, and zebrad_module (an unreleased commit that kept
# its last release's version): the version is the one their contracts name.
PATCHED = {
    'lez_core': 'apps/basecamp/common/deps/lez_core.lidl',
    'lez_indexer_module': 'apps/basecamp/common/deps/lez_indexer_module.lidl',
    'zebrad_module': 'apps/basecamp/common/deps/zebrad_module.lidl',
}
BLOCKCHAIN_VERSION = re.search(
    r'^readonly BLOCKCHAIN_MODULE_VERSION=(\S+)$',
    (ROOT / 'deploy/scripts/from-scratch.sh').read_text(), re.M).group(1)
# The Logos modules taken as released, at the versions the stack runs.
OFFICIAL = {'chat_module': ('0.2.2', ['delivery_module']), 'delivery_module': ('0.2.0', []),
            'blockchain_module': (BLOCKCHAIN_VERSION, [])}


def lidl_version(path):
    return re.search(r'^\s*version "([^"]+)"', (ROOT / path).read_text(), re.M).group(1)


def write_lgx(path, manifest):
    """A package as the builder lays it out: manifest.json and one variant."""
    with tarfile.open(path, 'w:gz') as t:
        for name, data in (('manifest.json', json.dumps(manifest, indent=2).encode()),
                           ('variants/linux-arm64/' + manifest['main']['linux-arm64'], b'\0plugin')):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))


def read_manifest(path):
    with tarfile.open(path, 'r:gz') as t:
        return json.load(t.extractfile('manifest.json'))


def chat_module(what):
    return subprocess.run(['bash', str(CHAT), '--print', what], check=True,
                          capture_output=True, text=True).stdout.strip()


def catalog_chat(name, names, versions):
    """The catalog's Chat module (package-chat-module.sh): Delivery pinned."""
    if name == 'chat_module':
        return [{'name': 'delivery_module', 'version': chat_module('delivery-requirement')}]
    return list(names)


def release_set(directory, dependencies_of=None, chat_version=None):
    """The release set's packages, each with the builder's bare names or with
    dependencies_of(name, names, versions) when given; chat_version replaces
    the Chat release's version (the catalog's set)."""
    versions = {n: json.loads((ROOT / p).read_text())['version'] for n, p in OURS.items()}
    versions.update({n: lidl_version(p) for n, p in PATCHED.items()})
    versions.update({n: v for n, (v, _) in OFFICIAL.items()})
    if chat_version:
        versions['chat_module'] = chat_version
    pathlib.Path(directory).mkdir(parents=True, exist_ok=True)
    paths = []
    for name, version in versions.items():
        if name in OURS:
            names = json.loads((ROOT / OURS[name]).read_text())['dependencies']
        else:
            names = OFFICIAL.get(name, (None, []))[1]
        deps = dependencies_of(name, names, versions) if dependencies_of else list(names)
        path = pathlib.Path(directory) / f'{name}.lgx'
        kind = 'ui_qml' if OURS.get(name, '').startswith('apps/') else 'core'
        write_lgx(path, {'name': name, 'version': version, 'type': kind, 'manifestVersion': '0.3.0',
                         'dependencies': deps, 'main': {'linux-arm64': f'{name}_plugin.so'},
                         'hashes': {'root': hashlib.sha256(name.encode()).hexdigest(),
                                    'variants/linux-arm64': hashlib.sha256(b'\0plugin').hexdigest()}})
        paths.append(path)
    return versions, paths


def v030_pins(name, names, versions):
    """What pin-package-dependencies.sh wrote at v0.3.0: a range, then the bare name."""
    if name not in OURS:
        return list(names)
    return [{'name': n, 'version': '~' + versions[n]} for n in names] + list(names)


def pin(paths, out, check=True):
    run = subprocess.run(['bash', str(PIN), '--out', str(out), *map(str, paths)], check=check,
                         capture_output=True, text=True)
    if not check:
        return run
    return {p.stem: read_manifest(p) for p in pathlib.Path(out).glob('*.lgx')}


class PinnedDependencies(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def check_set(self, versions, pinned):
        for name in OURS:
            deps = pinned[name]['dependencies']
            self.assertTrue(deps, name)
            # (2) every dependency once, and never as a bare name
            self.assertTrue(all(isinstance(d, dict) for d in deps), f'{name}: bare name in {deps}')
            listed = [d['name'] for d in deps]
            self.assertEqual(len(listed), len(set(listed)), f'{name} lists a dependency twice: {listed}')
            self.assertEqual(listed, json.loads((ROOT / OURS[name]).read_text())['dependencies'], name)
            for d in deps:
                if d['name'] in OFFICIAL and '-' not in versions[d['name']]:
                    # (4) the Logos modules we take unchanged: a patch release may stand in
                    self.assertEqual(d['version'], '~' + versions[d['name']], f'{name} -> {d}')
                else:
                    # (1) the patched upstream modules and (3) our own packages:
                    # exactly the version in the set, which no official release is
                    self.assertEqual(d['version'], versions[d['name']], f'{name} -> {d}')

    def test_the_builders_set_is_pinned_exactly_and_once(self):
        versions, paths = release_set(self.tmp / 'in')
        self.check_set(versions, pin(paths, self.tmp / 'out'))

    def test_the_v0_3_0_set_pins_again_to_the_same_manifests(self):
        versions, paths = release_set(self.tmp / 'in', v030_pins)
        again = pin(paths, self.tmp / 'again')
        self.check_set(versions, again)
        _, fresh = release_set(self.tmp / 'fresh')
        self.assertEqual(again, pin(fresh, self.tmp / 'out'))

    def test_the_patched_modules_are_required_exactly(self):
        versions, paths = release_set(self.tmp / 'in')
        pinned = pin(paths, self.tmp / 'out')
        required = {d['name']: d['version'] for n in OURS for d in pinned[n]['dependencies']}
        self.assertEqual(required['lez_core'], '0.4.1-sign.4')
        self.assertEqual(required['lez_indexer_module'], '1.1.1-rpc.4')
        self.assertEqual(required['zebrad_module'], '0.2.0')
        for name in PATCHED:
            self.assertNotRegex(required[name], r'^[~^<>=*]', name)

    def test_only_our_manifests_change(self):
        _, paths = release_set(self.tmp / 'in')
        out = self.tmp / 'out'
        pinned = pin(paths, out)
        for path in paths:
            name = path.stem
            if name not in OURS:
                self.assertEqual((out / path.name).read_bytes(), path.read_bytes(), name)
            self.assertEqual(pinned[name]['hashes'], read_manifest(path)['hashes'], name)
        rows = {r.split('\t')[0]: r.split('\t') for r in (out / 'packages.tsv').read_text().splitlines()[1:]}
        self.assertEqual(rows['lez_swap_node'][3], 'lez_core 0.4.1-sign.4')

    def test_the_catalogs_chat_module_is_required_exactly(self):
        # issue #147: Logos's chat_module 0.2.2 names a bare delivery_module,
        # which plans the newest Delivery (0.3.3) beside the desks' 0.2.x
        version = chat_module('version')
        self.assertTrue(version.startswith(OFFICIAL['chat_module'][0] + '-'), version)
        versions, paths = release_set(self.tmp / 'in', catalog_chat, chat_version=version)
        out = self.tmp / 'out'
        pinned = pin(paths, out)
        self.check_set(versions, pinned)
        for desk in ('lez_atomic_swap_maker', 'lez_atomic_swap_taker'):
            required = {d['name']: d['version'] for d in pinned[desk]['dependencies']}
            self.assertEqual(required['chat_module'], version, desk)
            # one Delivery requirement in the whole graph: the desk's is Chat's
            self.assertEqual(required['delivery_module'], chat_module('delivery-requirement'), desk)
        self.assertEqual((out / 'chat_module.lgx').read_bytes(), (self.tmp / 'in/chat_module.lgx').read_bytes())

    def test_two_requirements_of_one_name_are_refused(self):
        def chat_wants_another_delivery(name, names, versions):
            if name == 'chat_module':
                return [{'name': 'delivery_module', 'version': '>=0.3.0'}]
            return list(names)
        _, paths = release_set(self.tmp / 'in', chat_wants_another_delivery, chat_version='0.2.2-test.1')
        run = pin(paths, self.tmp / 'out', check=False)
        self.assertNotEqual(run.returncode, 0)
        self.assertIn('delivery_module is required in more than one way', run.stderr)

    def test_the_chat_module_is_cut_from_the_pinned_release_only(self):
        self.assertRegex(chat_module('source-sha256'), r'^[0-9a-f]{64}$')
        fake = self.tmp / 'chat_module-0.2.2.lgx'
        write_lgx(fake, {'name': 'chat_module', 'version': '0.2.2', 'dependencies': ['delivery_module'],
                         'main': {'linux-arm64': 'chat_module_plugin.so'}})
        run = subprocess.run(['bash', str(CHAT), '--from', str(fake), '--out', str(self.tmp / 'out.lgx')],
                             capture_output=True, text=True)
        self.assertEqual(run.returncode, 65, run.stderr)
        self.assertIn('not the chat_module 0.2.2 release', run.stderr)
        self.assertFalse((self.tmp / 'out.lgx').exists())

    @unittest.skipUnless(shutil.which('jq'), 'jq is required by stage-basecamp-package.sh')
    def test_the_images_install_the_bare_names_basecamp_0_2_reads(self):
        _, paths = release_set(self.tmp / 'in')
        pinned = pin(paths, self.tmp / 'out')
        deploy = self.tmp / 'deploy'
        (deploy / 'scripts').mkdir(parents=True)
        shutil.copy(STAGE, deploy / 'scripts')
        stage = deploy / 'scripts' / STAGE.name
        for name, kind in (('lez_atomic_swap_maker', 'maker'), ('lez_swap_node', 'module'),
                           ('lez_core', 'module')):
            subprocess.run(['bash', str(stage), str(self.tmp / 'out' / f'{name}.lgx'), kind, 'linux-arm64'],
                           check=True, capture_output=True, text=True)
        installed = {
            'lez_atomic_swap_maker': deploy / 'images/basecamp-ui/assets/maker-user/plugins/lez_atomic_swap_maker',
            'lez_swap_node': deploy / 'images/basecamp-ui/assets/bundle/modules/lez_swap_node',
            'lez_core': deploy / 'images/basecamp-ui/assets/bundle/modules/lez_core',
        }
        for name, directory in installed.items():
            manifest = json.loads((directory / 'manifest.json').read_text())
            expected = dict(pinned[name], dependencies=[d['name'] for d in pinned[name]['dependencies']])
            self.assertEqual(manifest, expected, name)
        self.assertEqual(json.loads((installed['lez_swap_node'] / 'manifest.json').read_text())['dependencies'],
                         ['lez_core'])


if __name__ == '__main__':
    unittest.main()
