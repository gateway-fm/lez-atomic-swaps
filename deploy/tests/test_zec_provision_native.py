"""zec-provision.sh --native-home: the corridor for Nodes that run natively
on this host (the launcher's macOS layout) is a Regtest corridor built with
the provisioners of this checkout. What it refuses before touching a chain;
the full run is in the evidence of native ZEC swaps."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


class NativeProvision(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        deploy = self.tmp / 'deploy'
        (deploy / 'scripts').mkdir(parents=True)
        (deploy / 'runtime').mkdir()
        (deploy / 'runtime' / 'runtime.env').write_text('LEZ_V02_CHANNEL_ID=' + 'e' * 64 + '\n')
        for script in ('zec-provision.sh', 'lane.sh', 'host.sh'):
            shutil.copy(SCRIPTS / script, deploy / 'scripts')
        self.deploy = deploy
        self.home = self.tmp / 'home'
        self.home.mkdir()

    def provision(self, *args, tools=None):
        # the scripts need bash >= 5.1 (host.sh): the one on PATH
        env = {'PATH': f"{pathlib.Path(shutil.which('bash')).parent}:/usr/bin:/bin:/usr/sbin:/sbin", 'HOME': str(self.tmp),
               'LEZ_ZEC_NATIVE_TOOLS': str(tools or self.tmp / 'no-tools')}
        return subprocess.run([shutil.which('bash'), 'scripts/zec-provision.sh', '--skip-chain-check', *args],
                              cwd=self.deploy, env=env, capture_output=True, text=True, timeout=60)

    def test_a_native_corridor_is_regtest_only(self):
        result = self.provision('--native-home', str(self.home), '--zcash-network', 'test')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('--native-home provisions a Regtest corridor only', result.stderr)

    def test_the_provisioners_must_be_built(self):
        result = self.provision('--native-home', str(self.home))
        self.assertEqual(result.returncode, 1)
        self.assertIn('zec-local-poc-provision is missing', result.stderr)

    def test_the_native_home_must_exist(self):
        result = self.provision('--native-home', str(self.tmp / 'absent'))
        self.assertEqual(result.returncode, 1)
        self.assertIn('is not a directory', result.stderr)


if __name__ == '__main__':
    unittest.main()
