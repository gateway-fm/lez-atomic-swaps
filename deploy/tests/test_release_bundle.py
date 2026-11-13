"""A release bundle's scripts take the images release.env names (#103 item 4).

package-dist.sh lays a bundle out as the deploy directory without images/,
builder/ or tests/, plus release.env beside compose.yaml. Only start.sh used to
read release.env, so a standalone `up.sh --fresh-lez` or `ui-e2e.sh` looked for
lez-*:local and tried to build. lane.sh, which every script that runs the stack
sources, now fills the image variables from it. Nothing here needs Docker.
"""
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'
RELEASE_ENV = '''# written by scripts/package-dist.sh; read by scripts/start.sh
LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez
LEZ_IMAGE_TAG=v0.3.1
LEZ_ESCROW_PROGRAM_ID={program}
LEZ_RELEASE_COMMIT=0123456789abcdef0123456789abcdef01234567
'''.format(program='ab' * 32)
NAMES = ('LEZ_IMAGE_PREFIX', 'LEZ_IMAGE_TAG', 'LEZ_ESCROW_PROGRAM_ID', 'LEZ_RELEASE_COMMIT', 'LEZ_IMAGES')
# What a script that resolves its images like up.sh does sees after sourcing lane.sh.
PROBE = '''set -euo pipefail
cd "$1"
source scripts/lane.sh
for name in LEZ_IMAGE_PREFIX LEZ_IMAGE_TAG LEZ_ESCROW_PROGRAM_ID LEZ_RELEASE_COMMIT LEZ_IMAGES; do
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
        deploy = tmp / 'lez-swap-stack-v0.3.1-arm64'
        (deploy / 'scripts').mkdir(parents=True)
        shutil.copy(SCRIPTS / 'lane.sh', deploy / 'scripts')
        shutil.copy(SCRIPTS / 'host.sh', deploy / 'scripts')
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
        self.assertEqual(seen['image'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-maker-node:v0.3.1')
        self.assertEqual(seen['child'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-taker-node:v0.3.1')
        self.assertEqual(seen['LEZ_ESCROW_PROGRAM_ID'], 'ab' * 32)
        self.assertEqual(seen['LEZ_RELEASE_COMMIT'], '0123456789abcdef0123456789abcdef01234567')
        self.assertEqual(seen['images'], 'pull')

    def test_a_bundle_of_local_images_uses_them_as_they_are(self):
        local = RELEASE_ENV.replace('LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez', 'LEZ_IMAGE_PREFIX=lez') \
                           .replace('LEZ_IMAGE_TAG=v0.3.1', 'LEZ_IMAGE_TAG=local')
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
        # Every harness script that resolves images sources lane.sh first.
        for name in ('up.sh', 'ui-e2e.sh', 'zec-provision.sh', 'bootstrap-market.sh', 'start.sh', 'reset-swaps.sh'):
            code = [line for line in (SCRIPTS / name).read_text().splitlines()
                    if not line.lstrip().startswith('#')]
            sourced = code.index('source scripts/lane.sh')
            uses = [i for i, line in enumerate(code) if 'LEZ_IMAGE_TAG' in line]
            self.assertTrue(all(sourced < i for i in uses), name)


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


if __name__ == '__main__':
    unittest.main()
