"""The release workflow publishes every image a bundle pulls, and carries every
file the role and Zebra images copy.

A bundle's start.sh pulls each image compose.yaml names; one the workflow never
pushes stops it at "manifest unknown" (the Zebra image was never pushed). The
Node, Taker and Zebra build contexts are filled from the rust job's upload, so a
file their Dockerfiles copy that the upload does not carry fails the image
build on the runner, never on a developer host (the ZEC provisioners).
Nothing here needs Docker.
"""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / '.github/workflows/release-images.yml').read_text()
COMPOSE = (ROOT / 'deploy/compose.yaml').read_text()


def pushed_images():
    images = set()
    for loop in re.findall(r'for image in ([a-z0-9 -]+); do\n\s*docker push', WORKFLOW):
        images.update(loop.split())
    return images


class ReleaseWorkflow(unittest.TestCase):
    def test_every_compose_image_is_pushed(self):
        named = set(re.findall(r'image: \$\{LEZ_IMAGE_PREFIX:-lez\}-([a-z0-9-]+):', COMPOSE))
        self.assertTrue(named)
        self.assertEqual(named - pushed_images(), set())

    def test_rust_job_uploads_what_the_contexts_copy(self):
        collect = WORKFLOW.split('name: node-payloads')[0].rsplit('- name: Collect payloads', 1)[1]
        roles = set(re.search(r'for role in ([a-z -]+); do', collect).group(1).split())
        for context in ('maker-node', 'taker-node', 'lez-services', 'zebra'):
            self.assertIn(context, roles, context)
        taker = (ROOT / 'deploy/images/taker-node/Dockerfile').read_text()
        for tool in re.findall(r'\bzec-local-poc-[a-z-]+', taker):
            self.assertIn('zec-local-poc-*', collect, tool)
        self.assertIn('source-tree.txt', collect)


class PackagesWorkflow(unittest.TestCase):
    """release-packages.yml: the catalog packages for linux-arm64 and
    linux-amd64, and darwin-arm64 for the packages a party's desk installs,
    merged and pinned, on manual dispatch only."""
    PACKAGES = (ROOT / '.github/workflows/release-packages.yml').read_text()

    def test_it_runs_only_on_manual_dispatch(self):
        on = self.PACKAGES.split('\non:\n', 1)[1].split('\n\n', 1)[0]
        self.assertEqual(on.strip(), 'workflow_dispatch:')

    def test_each_variant_builds_on_its_own_architecture(self):
        variants = dict(re.findall(r'- variant: (linux-[a-z0-9]+)\n\s+runner: (\S+)', self.PACKAGES))
        self.assertEqual(variants, {'linux-arm64': 'ubuntu-24.04-arm', 'linux-amd64': 'ubuntu-24.04'})

    def test_the_merged_set_is_pinned_before_the_catalog_assets_are_written(self):
        merge = self.PACKAGES.index('lgx merge')
        restore = self.PACKAGES.index('deploy/scripts/restore-merged-manifest.py "$f"')
        pin = self.PACKAGES.index('pin-package-dependencies.sh --out _pinned _merged/')
        self.assertLess(merge, restore)
        self.assertLess(restore, pin)
        assets = self.PACKAGES.index('"$dir/$name-$version.lgx"')
        self.assertLess(merge, pin)
        self.assertLess(pin, assets)
        self.assertIn('> "$dir/sidecar.json"', self.PACKAGES)

    def job(self, name):
        return self.PACKAGES.split(f'\n  {name}:\n', 1)[1].split('\n  merge:\n', 1)[0]

    def test_darwin_variants_build_on_apple_silicon_and_feed_the_merge(self):
        darwin = self.job('darwin')
        self.assertIn('runs-on: macos-14', darwin)
        self.assertIn('deploy/scripts/build-darwin-packages.sh --out _out', darwin)
        self.assertIn('name: lgx-darwin-arm64', darwin)
        self.assertIn('xcodebuild -downloadComponent MetalToolchain', darwin)
        self.assertIn('submodules: true', darwin)
        self.assertIn('needs: [variant, darwin]', self.PACKAGES)
        self.assertIn('"$name"-darwin-*.lgx', self.PACKAGES)

    def test_every_package_a_desk_installs_is_required_for_darwin(self):
        script = (ROOT / 'deploy/scripts/build-darwin-packages.sh').read_text()
        for built in ('one lez_core core', 'one lez_swap_node swap-node', 'for role in maker taker; do',
                      '"$out/lez_${role}_node-$variant.lgx"', 'one "lez_atomic_swap_$role" "desk-$role"'):
            self.assertIn(built, script)
        self.assertNotIn('lez_network', script.split('set -euo pipefail', 1)[1])
        # the verify step asks every published package for darwin-arm64 but these two
        linux_only = re.search(r'case "\$name" in ([a-z_|]+)\) requested="\$LINUX"', self.PACKAGES).group(1)
        self.assertEqual(set(linux_only.split('|')), {'lez_network', 'lez_indexer_module'})
        self.assertIn("ALL: '[\"darwin-arm64\",\"linux-amd64\",\"linux-arm64\"]'", self.PACKAGES)

    def test_the_lgx_that_verifies_reads_versioned_dependencies(self):
        # ff93a0df's lgx fails `lgx verify` on every pinned package
        flake = re.search(r'LGX_FLAKE: github:logos-co/logos-package/([0-9a-f]{40})#lgx', self.PACKAGES).group(1)
        self.assertNotEqual(flake, 'ff93a0df15ceab255f27687d22d962ea2737efbe')
        self.assertIn(flake, (ROOT / 'apps/basecamp/flake.lock').read_text())

    def test_actions_are_pinned_by_commit(self):
        for ref in re.findall(r'uses: (\S+)', self.PACKAGES):
            self.assertRegex(ref, r'@[0-9a-f]{40}$', ref)


if __name__ == '__main__':
    unittest.main()
