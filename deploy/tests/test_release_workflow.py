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
    linux-amd64, merged and pinned, on manual dispatch only."""
    PACKAGES = (ROOT / '.github/workflows/release-packages.yml').read_text()

    def test_it_runs_only_on_manual_dispatch(self):
        on = self.PACKAGES.split('\non:\n', 1)[1].split('\n\n', 1)[0]
        self.assertEqual(on.strip(), 'workflow_dispatch:')

    def test_each_variant_builds_on_its_own_architecture(self):
        variants = dict(re.findall(r'- variant: (linux-[a-z0-9]+)\n\s+runner: (\S+)', self.PACKAGES))
        self.assertEqual(variants, {'linux-arm64': 'ubuntu-24.04-arm', 'linux-amd64': 'ubuntu-24.04'})

    def test_the_merged_set_is_pinned_before_the_catalog_assets_are_written(self):
        merge = self.PACKAGES.index('lgx merge')
        pin = self.PACKAGES.index('pin-package-dependencies.sh --out _pinned _merged/')
        assets = self.PACKAGES.index('"$dir/$name-$version.lgx"')
        self.assertLess(merge, pin)
        self.assertLess(pin, assets)
        self.assertIn('> "$dir/sidecar.json"', self.PACKAGES)


if __name__ == '__main__':
    unittest.main()
