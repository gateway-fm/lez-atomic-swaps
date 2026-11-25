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
        # Monero's monerod too (profile `monero`): a bundle pulls it with LEZ_MONERO=1.
        self.assertIn('monerod', named)
        self.assertEqual(named - pushed_images(), set())

    def test_the_images_job_stages_and_builds_monero(self):
        images = WORKFLOW.split('\n  images:\n', 1)[1].split('\n  smoke:\n', 1)[0]
        stage = images.split('- name: Stage image contexts', 1)[1].split('- name:', 1)[0]
        self.assertIn('LEZ_MONERO: "1"', stage)
        build = re.search(r'docker compose (.*) build (.*)\n', images)
        self.assertIn('--profile monero', build.group(1))
        self.assertIn('monerod', build.group(2).split())

    def test_rust_job_uploads_what_the_contexts_copy(self):
        collect = WORKFLOW.split('name: node-payloads')[0].rsplit('- name: Collect payloads', 1)[1]
        roles = set(re.search(r'for role in ([a-z -]+); do', collect).group(1).split())
        for context in ('maker-node', 'taker-node', 'lez-services', 'zebra-module'):
            self.assertIn(context, roles, context)
        # the published Taker image carries zec-local-poc-identity and the
        # test-only payer zec-local-poc-transfer (feature test-payer), and none
        # of the dealer test fixture's tools
        self.assertIn('cp deploy/images/taker-node/zec-local-poc-identity '
                      'deploy/images/taker-node/zec-local-poc-transfer _out/images/taker-node/', collect)
        self.assertIn('install -m 0755 _in/node/images/taker-node/zec-local-poc-identity '
                      '_in/node/images/taker-node/zec-local-poc-transfer deploy/images/taker-node/', WORKFLOW)
        self.assertNotIn('zec-local-poc-*', WORKFLOW)
        for tool in ('provision', 'chat-draft', 'chat-finalize'):
            self.assertNotIn(f'zec-local-poc-{tool}', WORKFLOW, tool)
        self.assertIn('source-tree.txt', collect)
        # the Maker's Monero effect tools and both roles' Stage-A planner, which
        # package-role-node.sh puts into the Node packages
        self.assertIn('cp deploy/images/maker-node/xmr-reference-* _out/images/maker-node/', collect)
        self.assertIn('lez-v02-xmr-stage-a-plan', collect)
        # every file a Node package carries is one of those, of lez-*, or a
        # sidecar tool the collection names
        package = (ROOT / 'deploy/scripts/package-role-node.sh').read_text()
        packaged = set(re.findall(r'\b((?:lez|xmr)-[a-z0-9-]+)\b', package.split('case "$role" in', 1)[1].split('context=', 1)[0]))
        self.assertIn('xmr-reference-monero-fund', packaged)
        sidecar = set(re.search(r'sidecar/\{([^}]*)\}', collect).group(1).split(','))
        for tool in packaged:
            if tool.startswith('lez-v02-'):
                self.assertIn(tool, sidecar, tool)
            else:
                self.assertTrue(tool.startswith(('lez-', 'xmr-reference-')), tool)
        self.assertIn('install -m 0755 _in/node/images/maker-node/xmr-reference-* deploy/images/maker-node/', WORKFLOW)
        self.assertIn('chmod 0755 deploy/images/maker-node/xmr-reference-*', WORKFLOW)


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
    """The devnet's test hooks (feature test-hooks of the Taker Node, the Maker
    Node and the LEZ sidecar: a skewed clock, a cheating Taker, a held Zcash
    activation, a punish nonce thief) never
    reach a release: the devnet build enables them only on LEZ_TEST_HOOKS=1,
    nothing on the release path names them or sets that variable, the release
    path marks itself LEZ_RELEASE_BUILD, and under that mark from-scratch.sh
    refuses LEZ_TEST_HOOKS=1 and the hooks do not compile."""

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
        self.assertIn('hooks=",lez-taker-node/test-hooks,lez-maker-node/test-hooks"', text)
        self.assertIn('hooks="--features test-hooks"', text)
        self.assertIn('pair-xmr$LEZ_NODE_HOOKS_FEATURE"', text)
        # The release mark reaches both build containers.
        self.assertEqual(text.count('${release_build_env[@]+"${release_build_env[@]}"}'), 2)

    def test_the_crash_hooks_and_the_dealer_fixture_never_reach_a_release(self):
        # The release path builds neither feature, and under the release
        # mark neither compiles (the same const assert as test-hooks').
        for path in self.RELEASE_WORKFLOWS + self.PACKAGERS:
            text = (ROOT / path).read_text()
            for word in ('test-crash-hooks', '--features dealer-fixture'):
                self.assertNotIn(word, text, f'{path} names {word}')
        lib = (ROOT / 'crates/zec-reference-actor/src/lib.rs').read_text()
        for feature in ('test-crash-hooks', 'dealer-fixture'):
            self.assertRegex(
                lib,
                r'#\[cfg\(feature = "' + feature + r'"\)\]\nconst _: \(\) = assert!\(\n'
                r'    option_env!\("LEZ_RELEASE_BUILD"\)\.is_none\(\),',
                feature)
        # from-scratch.sh builds the dealer's tools only outside a release.
        text = self.FROM_SCRATCH.read_text()
        build = text.index('--features dealer-fixture')
        guard = text.rindex('if [[ -z "${LEZ_RELEASE_BUILD:-}" ]]; then', 0, build)
        self.assertNotIn('\n      fi\n', text[guard:build])

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
        taker = (ROOT / 'crates/taker-node/src/taker_service/xmr_supervisor.rs').read_text()
        module = taker.split('#[cfg(feature = "test-hooks")]\npub(super) mod test_hooks {', 1)[1]
        self.assertIn(guard, module.split('\n}\n', 1)[0])
        maker = (ROOT / 'crates/maker-node/src/actor_supervisor/runtime.rs').read_text()
        module = maker.split('#[cfg(feature = "test-hooks")]\npub(super) mod test_hooks {', 1)[1]
        self.assertIn(guard, module.split('\n}\n', 1)[0])
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
                    # Only the three crates' own definitions of the feature.
                    self.assertRegex(line, r'^test-hooks = \[("pair-xmr")?\]$', manifest)


class TestPayerInTheTakerImage(unittest.TestCase):
    """The Taker Node image, a release one too, carries the test payer
    zec-local-poc-transfer (feature test-payer of zec-reference-actor), with
    which the harness funds the Taker's Zcash key; no role package does.

    It is built in a cargo invocation of its own, so the feature never reaches
    the zec-reference-actor library the Nodes and actors link, and the dealer
    fixture, which enables it, still never builds in a release."""

    FROM_SCRATCH = (ROOT / 'deploy/scripts/from-scratch.sh').read_text()
    MANIFEST = (ROOT / 'crates/zec-reference-actor/Cargo.toml').read_text()

    def test_the_payer_is_its_own_feature_and_needs_no_dealer(self):
        self.assertIn('dealer-fixture = ["test-payer"]\n', self.MANIFEST)
        self.assertIn('test-payer = []\n', self.MANIFEST)
        payer = self.MANIFEST.split('name = "zec-local-poc-transfer"', 1)[1].split('[[bin]]', 1)[0]
        self.assertIn('required-features = ["test-payer"]', payer)
        # no Node, actor or other dependent asks for it
        for manifest in sorted(ROOT.glob('crates/*/Cargo.toml')):
            if manifest.parent.name != 'zec-reference-actor':
                self.assertNotIn('test-payer', manifest.read_text(), manifest)
        # and a release build does not refuse it as it refuses the dealer
        lib = (ROOT / 'crates/zec-reference-actor/src/lib.rs').read_text()
        self.assertNotRegex(lib, r'#\[cfg\(feature = "test-payer"\)\]\nconst _: \(\) = assert!')

    def test_from_scratch_builds_the_payer_alone_in_every_build(self):
        payer = 'cargo build --locked -p zec-reference-actor --features test-payer --bin zec-local-poc-transfer'
        self.assertEqual(self.FROM_SCRATCH.count(payer), 1)
        products = self.FROM_SCRATCH.index('cargo build --locked --bins --features "lez-maker-node/pair-zec')
        guard = self.FROM_SCRATCH.index('if [[ -z "${LEZ_RELEASE_BUILD:-}" ]]; then', products)
        at = self.FROM_SCRATCH.index(payer)
        self.assertLess(products, at)
        self.assertLess(at, guard)  # before the release-only guard: in every build
        dealer = self.FROM_SCRATCH[guard:self.FROM_SCRATCH.index('\n      fi\n', guard)]
        self.assertNotIn('zec-local-poc-transfer', dealer)
        self.assertIn('taker-node/zec-local-poc-identity|taker-node/zec-local-poc-transfer) ;;', self.FROM_SCRATCH)
        # the Taker image's context copies every zec-local-poc tool it is given
        self.assertIn('COPY zec-local-poc-* /payload/', (ROOT / 'deploy/images/taker-node/Dockerfile').read_text())

    def test_no_role_package_carries_it(self):
        for packager in ('deploy/scripts/package-role-node.sh', 'deploy/scripts/package-role-node-darwin.sh',
                         'deploy/scripts/build-darwin-packages.sh', 'deploy/role-symmetry.json'):
            self.assertNotIn('zec-local-poc', (ROOT / packager).read_text(), packager)
        self.assertNotIn('zec-local-poc', (ROOT / '.github/workflows/release-packages.yml').read_text())

class ReleaseImageTag(unittest.TestCase):
    """Only a run on a tag publishes under a release's name (issue #191).

    release-images.yml took a free-text image_tag on dispatch and used it as
    given, and check-release-ci.sh passes a branch dispatch, so a branch run
    with image_tag v0.3.4 would have pushed its images under the release's tag.
    scripts/release-image-tag.sh decides the tag; gh is a fake that knows the
    tags v0.3.6 and v0.4.0.
    """

    SCRIPT = ROOT / 'scripts/release-image-tag.sh'
    SHA = '957c8016' + '0' * 32
    FAKE_GH = '''#!/usr/bin/env bash
case "$2" in
  */git/ref/tags/v0.3.6|*/git/ref/tags/v0.4.0|*/git/ref/tags/sha-taken) echo "refs/tags/${2##*/git/ref/tags/}" ;;
  */git/ref/tags/sha-broken) echo "HTTP 500: server error" >&2; exit 1 ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac
'''

    def plan(self, ref_type, ref_name, image_tag='', event='workflow_dispatch'):
        import os
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            gh = pathlib.Path(tmp) / 'gh'
            gh.write_text(self.FAKE_GH)
            gh.chmod(0o755)
            env = {'PATH': f'{tmp}:{os.environ["PATH"]}', 'GITHUB_EVENT_NAME': event, 'GITHUB_REF_TYPE': ref_type,
                   'GITHUB_REF_NAME': ref_name, 'GITHUB_SHA': self.SHA, 'GITHUB_REPOSITORY': 'gateway-fm/lez-atomic-swaps',
                   'IMAGE_TAG_INPUT': image_tag}
            run = subprocess.run(['bash', str(self.SCRIPT)], env=env, capture_output=True, text=True, timeout=30)
        return run.returncode, run.stdout.strip(), run.stderr

    def test_a_release_publishes_under_its_tag(self):
        self.assertEqual(self.plan('tag', 'v0.4.0', event='release')[:2], (0, 'v0.4.0'))
        self.assertEqual(self.plan('tag', 'v0.4.0')[:2], (0, 'v0.4.0'))
        self.assertEqual(self.plan('tag', 'v0.4.0', 'v0.4.0')[:2], (0, 'v0.4.0'))

    def test_a_run_on_a_tag_publishes_under_no_other_name(self):
        code, _, err = self.plan('tag', 'v0.4.0', 'v0.3.6')
        self.assertEqual(code, 1)
        self.assertIn('publishes under v0.4.0 only', err)

    def test_a_branch_publishes_under_its_commit_by_default(self):
        self.assertEqual(self.plan('branch', 'main')[:2], (0, 'sha-957c80160000'))

    def test_a_branch_takes_a_namespaced_tag(self):
        self.assertEqual(self.plan('branch', 'fix/x', 'branch-fix-x')[:2], (0, 'branch-fix-x'))
        self.assertEqual(self.plan('branch', 'fix/x', 'sha-rehearsal.2')[:2], (0, 'sha-rehearsal.2'))

    def test_a_branch_never_publishes_under_a_release_name(self):
        for image_tag in ('v0.3.4', 'v0.4.0', 'v1.0.0-rc1', 'v0.3.6+build'):
            code, out, err = self.plan('branch', 'main', image_tag)
            self.assertEqual((code, out), (1, ''), image_tag)
            self.assertIn("is a release's name: only a run on the tag", err, image_tag)

    def test_a_branch_tag_must_be_namespaced(self):
        for image_tag in ('latest', 'm4-complete', 'v0', 'local', 'shax'):
            code, _, err = self.plan('branch', 'main', image_tag)
            self.assertEqual(code, 1, image_tag)
            self.assertIn('namespaced tag', err, image_tag)

    def test_a_branch_never_publishes_under_an_existing_tag(self):
        code, _, err = self.plan('branch', 'main', 'sha-taken')
        self.assertEqual(code, 1)
        self.assertIn('is the name of a tag of this repository', err)
        # the lookup fails closed
        code, _, err = self.plan('branch', 'main', 'sha-broken')
        self.assertEqual(code, 1)
        self.assertIn('cannot tell whether the tag sha-broken exists', err)

    def test_a_release_event_off_a_tag_is_refused(self):
        self.assertEqual(self.plan('branch', 'main', event='release')[0], 1)

    def test_the_workflow_decides_the_tag_with_the_script_before_any_build(self):
        gate = WORKFLOW[WORKFLOW.index('  ci-gate:'):WORKFLOW.index('  plan:')]
        self.assertIn('run: ./scripts/release-image-tag.sh', gate)
        self.assertIn('IMAGE_TAG_INPUT: ${{ inputs.image_tag }}', gate)
        plan = WORKFLOW[WORKFLOW.index('  plan:'):WORKFLOW.index('  rust:')]
        self.assertIn('tag="$(./scripts/release-image-tag.sh)"', plan)
        self.assertNotIn('INPUT_TAG', plan.replace('IMAGE_TAG_INPUT', ''))
        # every job that pushes takes the plan's tag
        for job in re.split(r'\n(?=  [a-z][a-z0-9-]*:\n)', WORKFLOW):
            if 'docker push' in job:
                self.assertRegex(job, r'needs: \[plan[,\]]')
                self.assertIn('LEZ_IMAGE_TAG: ${{ needs.plan.outputs.image_tag }}', job)

    def test_a_branch_run_of_the_packages_is_named_as_a_test_build(self):
        packages = (ROOT / '.github/workflows/release-packages.yml').read_text()
        self.assertIn("name: ${{ github.ref_type == 'tag' && 'catalog-packages' || "
                      "format('catalog-packages-test-{0}', github.sha) }}", packages)
        self.assertEqual(packages.count('name: catalog-packages'), 0)


if __name__ == '__main__':
    unittest.main()
