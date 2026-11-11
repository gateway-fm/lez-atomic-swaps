"""A release bundle's scripts take the images release.env names (#103 item 4).

package-dist.sh lays a bundle out as the deploy directory without images/,
builder/ or tests/, plus release.env beside compose.yaml. Only start.sh used to
read release.env, so a standalone `up.sh --fresh-lez` or `ui-e2e.sh` looked for
lez-*:local and tried to build. lane.sh, which every script that runs the stack
sources, now fills the image variables from it. Nothing here needs Docker.
"""
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'
RELEASE_ENV = '''# written by scripts/package-dist.sh; read by scripts/start.sh
LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez
LEZ_IMAGE_TAG=v0.3.0
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
        deploy = tmp / 'lez-swap-stack-v0.3.0-arm64'
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
        self.assertEqual(seen['image'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-maker-node:v0.3.0')
        self.assertEqual(seen['child'], 'ghcr.io/gateway-fm/lez-atomic-swaps/lez-taker-node:v0.3.0')
        self.assertEqual(seen['LEZ_ESCROW_PROGRAM_ID'], 'ab' * 32)
        self.assertEqual(seen['LEZ_RELEASE_COMMIT'], '0123456789abcdef0123456789abcdef01234567')
        self.assertEqual(seen['images'], 'pull')

    def test_a_bundle_of_local_images_uses_them_as_they_are(self):
        local = RELEASE_ENV.replace('LEZ_IMAGE_PREFIX=ghcr.io/gateway-fm/lez-atomic-swaps/lez', 'LEZ_IMAGE_PREFIX=lez') \
                           .replace('LEZ_IMAGE_TAG=v0.3.0', 'LEZ_IMAGE_TAG=local')
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


if __name__ == '__main__':
    unittest.main()
