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
        self.assertIn('runs-on: macos-15', darwin)
        self.assertIn('deploy/scripts/build-darwin-packages.sh --out _out', darwin)
        self.assertIn('name: lgx-darwin-arm64', darwin)
        self.assertIn('xcodebuild -downloadComponent MetalToolchain', darwin)
        self.assertIn('submodules: true', darwin)
        self.assertIn('needs: [variant, darwin]', self.PACKAGES)
        self.assertIn('"$name"-darwin-*.lgx', self.PACKAGES)

    def test_darwin_packages_land_where_the_upload_looks(self):
        # the job uploads _out from the checkout root; the script changes into
        # deploy/ first, so a relative --out is taken from where it was called
        script = (ROOT / 'deploy/scripts/build-darwin-packages.sh').read_text()
        caller = script.index('caller="$(pwd)"')
        self.assertLess(caller, script.index('cd "$(dirname "${BASH_SOURCE[0]}")/.."'))
        self.assertLess(script.index('[[ "$out" == /* ]] || out="$caller/$out"'), script.index('mkdir -p "$out"'))
        self.assertIn('path: _out', self.job('darwin'))

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

    def test_the_catalogs_chat_module_enters_the_pinned_set(self):
        # issue #147: the desks require package-chat-module.sh's Chat exactly;
        # the Chat built from its flake is not published
        merge = self.PACKAGES.split('\n  merge:\n', 1)[1]
        self.assertIn('deploy/scripts/package-chat-module.sh --out "$RUNNER_TEMP/chat_module.lgx"', merge)
        self.assertLess(merge.index('package-chat-module.sh'), merge.index('pin-package-dependencies.sh'))
        pin = merge.split('pin-package-dependencies.sh --out _pinned', 1)[1].split('\n          cat ', 1)[0]
        self.assertIn('"$RUNNER_TEMP/chat_module.lgx"', pin)
        self.assertNotIn('one chat_module', self.PACKAGES)
        for loop in re.findall(r'for name in (lez_atomic_swap_maker [a-z_ ]+); do', merge):
            self.assertIn('chat_module', loop.split(), loop)
        self.assertIn('requires a chat_module other than $chat', merge)

    def test_actions_are_pinned_by_commit(self):
        for ref in re.findall(r'uses: (\S+)', self.PACKAGES):
            self.assertRegex(ref, r'@[0-9a-f]{40}$', ref)



class NoTestHooksInARelease(unittest.TestCase):
    """The devnet's test hooks (feature test-hooks of the LEZ sidecar: the
    claim holds of zec-kill-maker-claim) never reach a release: the devnet
    build enables them only on LEZ_TEST_HOOKS=1, nothing on the release path
    names them or sets that variable, the release path marks itself
    LEZ_RELEASE_BUILD, and under that mark from-scratch.sh refuses
    LEZ_TEST_HOOKS=1 and the hooks do not compile."""

    RELEASE_WORKFLOWS = ('.github/workflows/release-images.yml', '.github/workflows/release-packages.yml')
    PACKAGERS = ('deploy/scripts/package-role-node.sh', 'deploy/scripts/package-role-node-darwin.sh',
                 'deploy/scripts/build-darwin-packages.sh', 'deploy/scripts/package-dist.sh',
                 'deploy/scripts/stage-basecamp-package.sh', 'deploy/scripts/pin-package-dependencies.sh')
    FROM_SCRATCH = ROOT / 'deploy/scripts/from-scratch.sh'

    def test_the_release_path_never_names_the_hooks(self):
        for path in self.RELEASE_WORKFLOWS + self.PACKAGERS:
            text = (ROOT / path).read_text()
            for word in ('test-hooks', 'test_hooks', 'LEZ_TEST_HOOKS', '--all-features'):
                self.assertNotIn(word, text, f'{path} names {word}')

    def test_the_release_workflows_mark_their_builds(self):
        for path in self.RELEASE_WORKFLOWS:
            text = (ROOT / path).read_text()
            top = text.split('\njobs:\n', 1)[0]
            self.assertRegex(top, r'\nenv:\n(  .*\n)*  LEZ_RELEASE_BUILD: "1"\n', path)
            # The builds they run are from-scratch.sh's phases, under that mark.
            self.assertIn('deploy/scripts/from-scratch.sh', text)

    def test_the_darwin_packager_marks_each_cargo_build(self):
        text = (ROOT / 'deploy/scripts/package-role-node-darwin.sh').read_text()
        builds = re.findall(r'.*cargo (?:\+\S+ )?build.*', text)
        self.assertEqual(len(builds), 2)
        for line in builds:
            self.assertIn('LEZ_RELEASE_BUILD=1 ', line)

    def test_from_scratch_builds_the_hooks_only_on_request(self):
        text = self.FROM_SCRATCH.read_text()
        for line in text.splitlines():
            if 'test-hooks' in line and not line.lstrip().startswith('#'):
                self.assertRegex(line, r'\[\[ "\$\{LEZ_TEST_HOOKS:-0\}" != 1 \]\] \|\| ', line)
        self.assertIn('hooks="--features test-hooks"', text)
        # The release mark reaches the sidecar's build container.
        self.assertEqual(text.count('${release_build_env[@]+"${release_build_env[@]}"}'), 1)

    def test_from_scratch_refuses_the_hooks_in_a_release_build(self):
        import os
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as workspace:
            for env, message in ((dict(LEZ_TEST_HOOKS='1', LEZ_RELEASE_BUILD='1'), 'no release build'),
                                 (dict(LEZ_TEST_HOOKS='yes'), 'LEZ_TEST_HOOKS is 0 or 1')):
                result = subprocess.run([str(self.FROM_SCRATCH), '--workspace', workspace, '--only', 'none'],
                                        env={**os.environ, **env}, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(message, result.stderr)

    def test_the_hooks_do_not_compile_in_a_release_build(self):
        guard = 'option_env!("LEZ_RELEASE_BUILD").is_none()'
        self.assertIn('#[cfg(feature = "test-hooks")]\nmod test_hooks;',
                      (ROOT / 'compat/lez-v0_2-sidecar/src/lib.rs').read_text())
        self.assertIn(guard, (ROOT / 'compat/lez-v0_2-sidecar/src/test_hooks.rs').read_text())

    def test_no_default_or_dependent_feature_turns_the_hooks_on(self):
        for manifest in sorted(ROOT.glob('**/Cargo.toml')):
            parts = manifest.relative_to(ROOT).parts
            if 'target' in parts or 'node_modules' in parts:
                continue
            text = manifest.read_text()
            default = re.search(r'^default = \[(.*?)\]', text, re.M | re.S)
            self.assertFalse(default and 'test-hooks' in default.group(1), manifest)
            for line in text.splitlines():
                if 'test-hooks' in line and not line.lstrip().startswith('#'):
                    # Only the sidecar's own definition of the feature.
                    self.assertEqual(line, 'test-hooks = []', manifest)


if __name__ == '__main__':
    unittest.main()
