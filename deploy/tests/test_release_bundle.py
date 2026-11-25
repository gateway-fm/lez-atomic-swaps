"""A release bundle's scripts take the images release.env names (#103 item 4).

package-dist.sh lays a bundle out as the deploy directory without images/,
builder/ or tests/, plus release.env beside compose.yaml. Only start.sh used to
read release.env, so a standalone `up.sh --fresh-lez` or `ui-e2e.sh` looked for
lez-*:local and tried to build. lane.sh, which every script that runs the stack
sources, now fills the image variables from it. Nothing here needs Docker.
"""
import importlib.util
import inspect
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'
RELEASE_ENV = '''# written by scripts/package-dist.sh; read by scripts/start.sh
LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez
LEZ_IMAGE_TAG=v0.4.0
LEZ_ESCROW_PROGRAM_ID={program}
LEZ_RELEASE_COMMIT=0123456789abcdef0123456789abcdef01234567
'''.format(program='ab' * 32)
NAMES = ('LEZ_IMAGE_PREFIX', 'LEZ_IMAGE_TAG', 'LEZ_ESCROW_PROGRAM_ID', 'LEZ_RELEASE_COMMIT', 'LEZ_IMAGES', 'LEZ_RELEASE_BUNDLE')
# What a script that resolves its images like up.sh does sees after sourcing lane.sh.
PROBE = '''set -euo pipefail
cd "$1"
source scripts/lane.sh
for name in LEZ_IMAGE_PREFIX LEZ_IMAGE_TAG LEZ_ESCROW_PROGRAM_ID LEZ_RELEASE_COMMIT LEZ_IMAGES LEZ_RELEASE_BUNDLE; do
  printf '%s=%s\\n' "$name" "${!name:-}"
done
printf 'image=%s\\n' "${LEZ_IMAGE_PREFIX:-lez}-maker-node:${LEZ_IMAGE_TAG:-local}"
printf 'images=%s\\n' "${LEZ_IMAGES:-build}"
bash -c 'printf "child=%s\\n" "${LEZ_IMAGE_PREFIX:-lez}-taker-node:${LEZ_IMAGE_TAG:-local}"'
'''


class ReleaseBundle(unittest.TestCase):
    def deploy_dir(self, release_env):
        tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        deploy = tmp / 'lez-swap-stack-v0.4.0-arm64'
        (deploy / 'scripts').mkdir(parents=True)
        for name in ('lane.sh', 'host.sh', 'release-env.sh'):
            shutil.copy(SCRIPTS / name, deploy / 'scripts')
        (deploy / 'compose.yaml').write_text('name: ${LEZ_STACK_PREFIX:-lez}-swap-stack\n')
        if release_env is not None:
            (deploy / 'release.env').write_text(release_env)
        return deploy

    def probe(self, deploy, **env):
        clean = {key: value for key, value in os.environ.items() if not key.startswith('LEZ_')}
        run = subprocess.run(['bash', '-c', PROBE, '-', str(deploy)], env={**clean, **env},
                             capture_output=True, text=True, check=True)
        return dict(line.split('=', 1) for line in run.stdout.splitlines())

    def test_a_bundle_resolves_the_release_images_and_never_builds(self):
        seen = self.probe(self.deploy_dir(RELEASE_ENV))
        self.assertEqual(seen['image'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-maker-node:v0.4.0')
        self.assertEqual(seen['child'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-taker-node:v0.4.0')
        self.assertEqual(seen['LEZ_ESCROW_PROGRAM_ID'], 'ab' * 32)
        self.assertEqual(seen['LEZ_RELEASE_COMMIT'], '0123456789abcdef0123456789abcdef01234567')
        self.assertEqual(seen['images'], 'pull')
        self.assertEqual(seen['LEZ_RELEASE_BUNDLE'], '1')

    def test_a_bundle_of_local_images_uses_them_as_they_are(self):
        local = RELEASE_ENV.replace('LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez', 'LEZ_IMAGE_PREFIX=lez') \
                           .replace('LEZ_IMAGE_TAG=v0.4.0', 'LEZ_IMAGE_TAG=local')
        seen = self.probe(self.deploy_dir(local))
        self.assertEqual(seen['image'], 'lez-maker-node:local')
        self.assertEqual(seen['images'], 'present')

    def test_the_environment_wins_over_release_env(self):
        seen = self.probe(self.deploy_dir(RELEASE_ENV), LEZ_IMAGE_TAG='v0.3.1-rc1', LEZ_IMAGES='present')
        self.assertEqual(seen['image'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-maker-node:v0.3.1-rc1')
        self.assertEqual(seen['images'], 'present')

    def test_a_checkout_without_release_env_is_unchanged(self):
        seen = self.probe(self.deploy_dir(None))
        for name in NAMES:
            self.assertEqual(seen[name], '', name)
        self.assertEqual(seen['image'], 'lez-maker-node:local')
        self.assertEqual(seen['images'], 'build')

    def test_package_dist_lays_out_the_bundle_this_test_models(self):
        # The bundle is `git archive HEAD:deploy` without the build material,
        # plus release.env beside compose.yaml with these four keys.
        script = (SCRIPTS / 'package-dist.sh').read_text()
        self.assertIn('git -C "$REPO_ROOT" archive --format=tar HEAD:deploy', script)
        self.assertIn('rm -rf "$stage/$name/builder" "$stage/$name/images" "$stage/$name/tests"', script)
        self.assertIn('cat >"$stage/$name/release.env" <<ENV', script)
        for name in NAMES[:4]:
            self.assertIn(f'{name}=${{', script, name)
        # Every harness script that resolves images sources lane.sh first, or
        # release-env.sh, which lane.sh sources, where it sources lane.sh only
        # in a subshell (regress.sh) or only for a lane >= 2 (gen-config.sh).
        sources = {'regress.sh': 'source scripts/release-env.sh',
                   'gen-config.sh': 'source "$DEPLOY_ROOT/scripts/release-env.sh"'}
        for name in ('up.sh', 'ui-e2e.sh', '../tests/fixtures/zec-catalog-fixture.sh', 'bootstrap-market.sh', 'start.sh',
                     'reset-swaps.sh', 'regress.sh', 'gen-config.sh'):
            code = [line for line in (SCRIPTS / name).read_text().splitlines()
                    if not line.lstrip().startswith('#')]
            sourced = code.index(sources.get(name, 'source scripts/lane.sh'))
            uses = [i for i, line in enumerate(code) if 'LEZ_IMAGE_TAG' in line or 'LEZ_ESCROW_PROGRAM_ID' in line]
            self.assertTrue(all(sourced < i for i in uses), name)
        self.assertIn('source "$(dirname "${BASH_SOURCE[0]}")/release-env.sh"', (SCRIPTS / 'lane.sh').read_text())


def code_lines(name):
    """The script's commands: comments dropped, continuation lines joined."""
    joined, lines = '', []
    for line in (SCRIPTS / name).read_text().splitlines():
        if line.lstrip().startswith('#'):
            continue
        if line.endswith('\\'):
            joined += line[:-1] + ' '
            continue
        lines.append(joined + line)
        joined = ''
    return lines


# The fake docker records each call; every other command is the host's own.
FAKE_DOCKER = '''#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$DOCKER_LOG"
'''


class BundleStartBuildsNothing(unittest.TestCase):
    """start.sh on a bundle starts the images it pulled and builds nothing.

    The release smoke job (run 43189324839) died after the market bootstrap:
    start.sh restarted the Nodes with `docker compose up` without --no-build.
    The role images name another service as a build context
    (`basecamp: service:basecamp-ui`), and for those compose builds on every
    `up`; the bundle has no images/basecamp-ui, so the build could not even
    load its context. Every `compose up` start.sh reaches must pass --no-build,
    and every image it starts must be one it pulled.
    """

    def test_start_and_up_never_build(self):
        for name in ('start.sh', 'up.sh'):
            for line in code_lines(name):
                # a command, not the banner's hint that quotes one for the reader
                if not re.search(r'(^|\|\||&&|\bthen|\bdo)\s*docker compose\b', line.strip()):
                    continue
                words = line.split()
                if 'up' in words:
                    self.assertIn('--no-build', line, f'{name}: {line.strip()}')
                self.assertNotIn('--build', words, f'{name}: {line.strip()}')
        # start.sh builds nothing at all; up.sh only in its LEZ_IMAGES=build branch.
        build = re.compile(r'docker compose\b.*\sbuild\b')
        self.assertEqual([l for l in code_lines('start.sh') if build.search(l)], [])
        up = code_lines('up.sh')
        builds = [i for i, l in enumerate(up) if build.search(l)]
        branch = up.index('  build)')
        branch_end = next(i for i in range(branch, len(up)) if up[i].strip() == ';;')
        self.assertTrue(builds)
        self.assertTrue(all(branch < i < branch_end for i in builds), [up[i] for i in builds])

    def test_the_bundle_pulls_every_image_start_sh_starts(self):
        # verify-all.sh starts the explorers (profile "explorers") with
        # --no-build on a bundle, so the pulls must fetch them as well.
        for name in ('start.sh', 'up.sh'):
            pulls = [l for l in code_lines(name) if re.search(r'docker compose\b.*\spull\b', l)]
            self.assertTrue(pulls, name)
            for line in pulls:
                for profile in ('tools', 'explorers'):
                    self.assertIn(f'--profile {profile}', line, f'{name}: {line.strip()}')
                # and Monero's image when the stack runs it (LEZ_MONERO=1)
                self.assertIn('${monero_profile[@]+"${monero_profile[@]}"}', line, f'{name}: {line.strip()}')
            self.assertIn('monero_profile=(--profile monero)', '\n'.join(code_lines(name)), name)

    def run_verify_all(self, release_env, checkout=False, **env):
        deploy = ReleaseBundle.deploy_dir(self, release_env)
        if checkout:  # a checkout has the build contexts a bundle does not ship
            for name in ('btc-explorer', 'lez-explorer'):
                (deploy / 'images' / name).mkdir(parents=True)
        shutil.copy(SCRIPTS / 'verify-all.sh', deploy / 'scripts')
        (deploy / 'runtime').mkdir()
        (deploy / 'runtime' / 'runtime.env').write_text('LEZ_STACK_PREFIX=lez\n')
        fake = deploy.parent / 'bin'
        fake.mkdir()
        for tool, body in (('docker', FAKE_DOCKER), ('curl', '#!/usr/bin/env bash\nexit 0\n')):
            (fake / tool).write_text(body)
            (fake / tool).chmod(0o755)
        log = deploy.parent / 'docker.log'
        clean = {key: value for key, value in os.environ.items() if not key.startswith('LEZ_')}
        subprocess.run(['bash', str(deploy / 'scripts' / 'verify-all.sh')],
                       env={**clean, **env, 'PATH': f'{fake}:{clean["PATH"]}', 'DOCKER_LOG': str(log)},
                       stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=120)
        return [line for line in log.read_text().splitlines() if line.startswith('compose ')]

    def test_verify_all_starts_the_explorers_without_a_build_on_a_bundle(self):
        ups = [l for l in self.run_verify_all(RELEASE_ENV) if 'up' in l.split()]
        self.assertTrue(ups)
        for line in ups:
            self.assertIn('--no-build', line.split(), line)

    def test_verify_all_on_a_checkout_is_unchanged(self):
        # Also on the checkout's own images (start.sh with the tag `local` sets
        # LEZ_IMAGES=present): up.sh neither builds nor checks the explorers'
        # images, so verify-all.sh is where a checkout first builds them.
        for env in ({}, {'LEZ_IMAGES': 'present'}):
            ups = [l for l in self.run_verify_all(None, checkout=True, **env) if 'up' in l.split()]
            self.assertTrue(ups, env)
            for line in ups:
                self.assertNotIn('--no-build', line.split(), f'{env}: {line}')

    def test_verify_all_builds_nothing_where_there_is_no_build_context(self):
        # No release.env and no images/: nothing here could be built either.
        for env in ({}, {'LEZ_IMAGES': 'present'}):
            ups = [l for l in self.run_verify_all(None, **env) if 'up' in l.split()]
            self.assertTrue(ups, env)
            for line in ups:
                self.assertIn('--no-build', line.split(), f'{env}: {line}')

    def test_verify_all_builds_nothing_on_pulled_images_in_a_checkout(self):
        ups = [l for l in self.run_verify_all(None, checkout=True, LEZ_IMAGES='pull') if 'up' in l.split()]
        self.assertTrue(ups)
        for line in ups:
            self.assertIn('--no-build', line.split(), line)

    def test_every_built_service_names_an_image_release_env_selects(self):
        # A service compose could build has an image of the release's prefix
        # and tag, so a bundle's pull and --no-build resolve it.
        compose = (SCRIPTS.parent / 'compose.yaml').read_text()
        services = re.split(r'\n(?=  [a-z][a-z0-9-]*:\n)', compose)
        built = [s for s in services if re.search(r'^    build:', s, re.M)]
        self.assertGreaterEqual(len(built), 10)
        for service in built:
            self.assertRegex(service, r'\n    image: \$\{LEZ_IMAGE_PREFIX:-lez\}-[a-z-]+:\$\{LEZ_IMAGE_TAG:-local\}\n',
                             service.splitlines()[0])



DEPLOY = SCRIPTS.parent
REPO = DEPLOY.parent
HOOK_CHECKS = ('require_sidecar_test_hooks(', 'xmr_require_test_hooks(', 'require_maker_node_test_hooks(')


def bundle_tree(root):
    """The tree package-dist.sh lays out, from the working tree: deploy/'s files
    git knows (tracked, or new and not ignored), less what package-dist.sh
    removes, plus release.env. Docker is never needed to build it."""
    script = (SCRIPTS / 'package-dist.sh').read_text()
    rm = re.search(r'^rm -rf ("\$stage/\$name/builder".*?)(?<!\\)$', script, re.M | re.S).group(1)
    removed = re.findall(r'"\$stage/\$name/([^"]+)"', rm)
    self_check = {'builder', 'images', 'tests', 'scripts/package-dist.sh'}
    assert self_check <= set(removed), removed
    listed = subprocess.run(['git', '-C', str(REPO), 'ls-files', '-z', '--cached', '--others', '--exclude-standard',
                             '--', 'deploy'], capture_output=True, check=True).stdout.decode().split('\0')
    tree = root / 'lez-swap-stack-v0.4.0-arm64'
    for name in filter(None, listed):
        relative = pathlib.PurePosixPath(name).relative_to('deploy')
        if any(relative == pathlib.PurePosixPath(r) or pathlib.PurePosixPath(r) in relative.parents for r in removed):
            continue
        source = REPO / name
        if not source.is_file():
            continue  # deleted in the working tree
        (tree / relative).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, tree / relative)
    (tree / 'release.env').write_text(RELEASE_ENV)
    return tree


class BundleRegressionEntryPoints(unittest.TestCase):
    """The regression's entry points in a release bundle, up to Docker (#190).

    The reviewer's v0.3.6 bundle run needed local edits: the custody-credit
    scenarios read no release.env (escrow_program_id() looked for
    ../scripts/verify-lez-v02-provisional.sh, which a bundle lacks), gen-config.sh
    fell back to lez-network-module:local, and the scenarios that need the devnet's
    test hooks failed obscurely. In a tree laid out as package-dist.sh lays it out,
    the entry points take release.env and name what release images cannot run.
    """

    @classmethod
    def setUpClass(cls):
        cls.tmp = pathlib.Path(tempfile.mkdtemp())
        cls.tree = bundle_tree(cls.tmp)
        (cls.tree / 'runtime').mkdir()
        (cls.tree / 'runtime' / 'runtime.env').write_text('LEZ_STACK_PREFIX=lez\nLEZ_TIMING_PROFILE=fast\n')
        cls.bin = cls.tmp / 'bin'
        cls.bin.mkdir()
        (cls.bin / 'docker').write_text(FAKE_DOCKER)
        (cls.bin / 'docker').chmod(0o755)
        cls.docker_log = cls.tmp / 'docker.log'

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)

    def env(self, **extra):
        clean = {key: value for key, value in os.environ.items() if not key.startswith('LEZ_')}
        return {**clean, 'PATH': f'{self.bin}:{clean["PATH"]}', 'DOCKER_LOG': str(self.docker_log), **extra}

    def run_in(self, cwd, *argv, **extra):
        return subprocess.run(list(argv), cwd=cwd, env=self.env(**extra), capture_output=True, text=True,
                              stdin=subprocess.DEVNULL, timeout=120)

    def load_harness(self, deploy):
        clean = {key: value for key, value in os.environ.items() if not key.startswith('LEZ_')}
        with unittest.mock.patch.dict(os.environ, clean, clear=True):
            spec = importlib.util.spec_from_file_location('node_e2e_bundle', deploy / 'scripts' / 'node-e2e.py')
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            environment = dict(os.environ)
        return module, environment

    def test_the_tree_is_a_bundle(self):
        self.assertFalse((self.tree / 'images').exists())
        self.assertFalse((self.tree / 'tests').exists())
        self.assertFalse((self.tree.parent / 'scripts' / 'verify-lez-v02-provisional.sh').exists())
        self.assertTrue((self.tree / 'scripts' / 'release-env.sh').is_file())

    def test_the_harness_takes_release_env_in_a_bundle(self):
        harness, environment = self.load_harness(self.tree)
        self.assertTrue(harness.RELEASE_BUNDLE)
        self.assertEqual(harness.escrow_program_id(), 'ab' * 32)
        # what it passes to compose, gen-config.sh and the fixtures it starts
        self.assertEqual(environment['LEZ_IMAGE_PREFIX'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez')
        self.assertEqual(environment['LEZ_IMAGE_TAG'], 'v0.4.0')
        self.assertEqual(environment['LEZ_ESCROW_PROGRAM_ID'], 'ab' * 32)
        self.assertEqual(environment['LEZ_IMAGES'], 'present')

    def test_a_checkout_harness_is_unchanged(self):
        harness, environment = self.load_harness(DEPLOY)
        self.assertFalse(harness.RELEASE_BUNDLE)
        self.assertNotIn('LEZ_IMAGE_TAG', environment)
        self.assertNotIn('LEZ_IMAGES', environment)
        self.assertRegex(harness.escrow_program_id(), r'^[0-9a-f]{64}$')

    def test_gen_config_takes_release_env(self):
        # gen-config.sh sources release-env.sh itself: run on its own, or from
        # node-e2e.py's regenerated-config, it names the release's network image.
        probe = ('set -euo pipefail; cd "$1"; source scripts/release-env.sh; '
                 'printf "%s\\n" "${LEZ_IMAGE_PREFIX:-lez}-network-module:${LEZ_IMAGE_TAG:-local}"')
        seen = self.run_in(self.tree, 'bash', '-c', probe, '-', str(self.tree))
        self.assertEqual(seen.stdout.strip(), 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-network-module:v0.4.0', seen.stderr)

    def release_unavailable(self, cwd, **extra):
        listed = self.run_in(cwd, sys.executable, '-I', 'scripts/node-e2e.py', 'release-unavailable', **extra)
        self.assertEqual(listed.returncode, 0, listed.stderr)
        return [tuple(line.split('\t', 1)) for line in listed.stdout.splitlines()]

    def test_release_unavailable_names_every_scenario_that_needs_the_hooks_or_the_dealer(self):
        entries = self.release_unavailable(self.tree)
        patterns = [pattern for pattern, _ in entries]
        harness, _ = self.load_harness(self.tree)
        named = {name for pattern in patterns for name in [pattern.split(':')[2]] if name != '*'}

        def body(function):
            source = inspect.getsource(function)
            called = re.findall(r'\b(scenario_\w+|through_taker_cli)\(', source) if source.lstrip().startswith(('"', "'")) or 'lambda' in source else []
            return source + ''.join(inspect.getsource(getattr(harness, c)) for c in called if hasattr(harness, c))

        hooked = {name for table in (harness.SCENARIOS, harness.ZEC_SCENARIOS, harness.XMR_SCENARIOS)
                  for name, function in table.items() if any(c in body(function) for c in HOOK_CHECKS)}
        # scenario_xmr_kill_walk checks the hooks for the Maker's walk only
        self.assertEqual(hooked - {'xmr-kill-taker'}, {
            'xmr-taker-clock-ahead', 'xmr-punish-replans', 'xmr-taker-cheats', 'xmr-kill-maker',
            'zec-kill-maker-claim', 'zec-activate-after-expiry'})
        self.assertLessEqual(hooked - {'xmr-kill-taker'}, named)
        self.assertLessEqual(harness.ZEC_CATALOG_SCENARIOS | harness.ZEC_DEALER_LAYOUT_SCENARIOS, named)
        self.assertIn('maker-price-module', named)
        self.assertIn('*:zec:*:TakerSellsForeign', patterns)
        for _, why in entries:
            self.assertTrue(why.strip())
        # the README names the same set
        readme = (DEPLOY / 'README.md').read_text()
        section = readme[readme.index('**The regression from a bundle.**'):readme.index('Upgrading from one release')]
        for pattern in patterns:
            if pattern.split(':')[2] != '*':
                self.assertIn(pattern.removesuffix(':*'), section, pattern)
        self.assertIn('node:zec:*:TakerSellsForeign', section)
        self.assertIn('ui:zec:*:TakerSellsForeign', section)

    def test_dealer_agreements_make_every_zcash_test_unavailable(self):
        patterns = [p for p, _ in self.release_unavailable(self.tree, LEZ_ZEC_AGREEMENT='dealer')]
        self.assertIn('*:zec:*:*', patterns)

    def regress_list(self, cwd, tier='full'):
        listed = self.run_in(cwd, 'bash', 'scripts/regress.sh', 'list', tier)
        self.assertEqual(listed.returncode, 0, listed.stderr)
        return listed.stdout.split(), listed.stderr.splitlines()

    def test_regress_leaves_them_out_of_a_bundle_and_says_why(self):
        everything, said = self.regress_list(DEPLOY)
        self.assertEqual(said, [])
        kept, said = self.regress_list(self.tree)
        left_out = [t for t in everything if t not in kept]
        self.assertEqual(kept, [t for t in everything if t not in left_out])
        for test in ('node:zec:zec-kill-maker-claim:TakerSellsLez', 'node:zec:zec-activate-after-expiry:TakerSellsLez',
                     'node:xmr:xmr-taker-cheats:TakerSellsLez', 'node:xmr:xmr-kill-maker:TakerSellsLez',
                     'node:zec:zec-regenerated-catalog:TakerSellsLez', 'node:zec:zec-maker-early-claim:TakerSellsLez',
                     'node:btc:maker-price-module:TakerSellsForeign', 'node:zec:zec-happy:TakerSellsForeign',
                     'ui:zec:zec-happy:TakerSellsForeign'):
            self.assertIn(test, left_out)
            line = next(line for line in said if f'=== {test}: ' in line)
            self.assertIn('not run on release images (', line)
        self.assertEqual(len(said), len(left_out))
        for test in ('node:zec:zec-happy:TakerSellsLez', 'node:btc:btc-custody-credit:TakerSellsForeign',
                     'node:zec:zec-custody-credit:TakerSellsLez', 'node:btc:regenerated-config:TakerSellsForeign',
                     'node:xmr:xmr-kill-taker:TakerSellsLez', 'ui:btc:happy:TakerSellsLez'):
            self.assertIn(test, kept)
        self.assertEqual(self.regress_list(self.tree, 'smoke')[0],
                         [t for t in self.regress_list(DEPLOY, 'smoke')[0] if not t.startswith(('node:zec', 'ui:zec')) or
                          not t.endswith('TakerSellsForeign')])

    def test_the_harness_refuses_one_by_name_before_it_reaches_a_node(self):
        self.docker_log.unlink(missing_ok=True)
        refused = self.run_in(self.tree, sys.executable, '-I', 'scripts/node-e2e.py', '--pair', 'Zcash', '--direction',
                              'TakerSellsLez', 'zec-kill-maker-claim', LEZ_CONTAINER_PREFIX='lez')
        self.assertEqual(refused.returncode, 1)
        self.assertIn('zec-kill-maker-claim TakerSellsLez is not run on release images: it needs the devnet test hooks',
                      refused.stdout)
        self.assertFalse(self.docker_log.exists() and self.docker_log.read_text().strip(), 'a Node was reached')


# Every harness script a bundle carries that starts a container; from-scratch.sh,
# stage-assets.sh and the like are not in a bundle (package-dist.sh removes them).
def bundle_harness_files():
    removed = {'from-scratch.sh', 'stage-assets.sh', 'stage-basecamp-package.sh', 'package-dist.sh'}
    files = [p for p in sorted(SCRIPTS.iterdir()) if p.suffix in ('.sh', '.py') and p.name not in removed]
    return files + sorted((DEPLOY / 'tests' / 'fixtures').glob('*.sh'))


class BundleHarnessBuildsNothing(unittest.TestCase):
    """No `docker compose up` the harness reaches builds, and none of its `compose
    run`s is of a service built on another service (#190). For those, compose builds
    before every up and every run; the bundle has no images/ to build from.
    zec-catalog-fixture.sh recreated the Nodes without --no-build (its error went
    to /dev/null), and ui-e2e.sh joined its videos with `compose run maker-node`."""

    def commands(self, path):
        """The file's commands: comments and here-documents (a banner) dropped,
        continuation lines joined."""
        lines, joined, heredoc = [], '', None
        for line in path.read_text().splitlines():
            if heredoc is not None:
                heredoc = None if line.strip() == heredoc else heredoc
                continue
            if line.lstrip().startswith('#'):
                continue
            if line.endswith('\\'):
                joined += line[:-1] + ' '
                continue
            lines.append(joined + line)
            joined = ''
            opened = re.search(r"<<-?'?([A-Z]+)'?\s*$", line) if path.suffix == '.sh' else None
            heredoc = opened.group(1) if opened else None
        return lines

    def test_every_compose_up_passes_no_build(self):
        checked = 0
        for path in bundle_harness_files():
            for line in self.commands(path):
                if path.suffix == '.py':
                    if '"up"' in line and ('"compose"' in line or '*compose' in line):
                        checked += 1
                        self.assertIn('"--no-build"', line, f'{path.name}: {line.strip()}')
                    continue
                # a command (not a message or the banner's hint that quotes one)
                if not re.search(r'(^|\|\||&&|\$\(|\bthen|\bdo|;)\s*(docker compose|"\$\{compose\[@\]\}")\s.*\bup\b',
                                 line.strip()):
                    continue
                checked += 1
                self.assertIn('--no-build', line.split(), f'{path.name}: {line.strip()}')
        self.assertGreaterEqual(checked, 15)

    def test_no_compose_run_of_a_service_built_on_another(self):
        compose = (DEPLOY / 'compose.yaml').read_text()
        services = re.split(r'\n(?=  [a-z][a-z0-9-]*:\n)', compose)
        on_another = {s.split(':', 1)[0].strip() for s in services if 'service:basecamp-ui' in s}
        self.assertEqual(on_another, {'network', 'maker-node', 'taker-node'})
        for path in bundle_harness_files():
            for line in self.commands(path):
                match = re.search(r'compose\b.*\brun\b(.*)', line)
                if not match or path.suffix == '.py':
                    continue
                words = set(match.group(1).replace('"', ' ').split())
                self.assertFalse(words & on_another | {w for w in words if w.endswith('-node') and '$' in w},
                                 f'{path.name}: {line.strip()}')

    def test_compose_build_only_where_images_are_built(self):
        # up.sh builds in its LEZ_IMAGES=build branch (BundleStartBuildsNothing),
        # bootstrap-market.sh only when LEZ_IMAGES is build.
        build = re.compile(r'docker compose\b.*\sbuild\b')
        for path in bundle_harness_files():
            if path.name == 'up.sh':
                continue
            for index, line in enumerate(lines := self.commands(path)):
                if not build.search(line) or 'rebuild its image' in line:
                    continue
                self.assertEqual(path.name, 'bootstrap-market.sh', line)
                self.assertIn('[[ "${LEZ_IMAGES:-build}" == build ]] ||', '\n'.join(lines[max(0, index - 3):index]))

    def test_a_failed_video_join_keeps_its_segments(self):
        join = (SCRIPTS / 'ui-e2e.sh').read_text()
        body = join[join.index('join_video() {'):]
        body = body[:body.index('\n}\n')]
        self.assertIn('docker run --rm --network none', body)
        code = '\n'.join(line for line in body.splitlines() if not line.lstrip().startswith('#'))
        self.assertNotIn('compose', code)
        ok, failed = body.index('; then'), body.index('  else')
        self.assertLess(ok, body.index('rm -f "$videos/$seg.mp4"'))
        self.assertLess(body.index('rm -f "$videos/$seg.mp4"'), failed)
        self.assertNotIn('rm -f', body[failed:])
        self.assertIn('${error:-no output}', body[failed:])


if __name__ == '__main__':
    unittest.main()
