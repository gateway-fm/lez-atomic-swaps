"""Failure and replay contracts for the reviewer entry points; no Docker needed."""
import importlib.util
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    obj = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(obj)
    return obj


seed = module('seed', 'seed-btc-wallets.py')
rec = module('rec', 'record-reviewer-evidence.py')


class WalletRPC:
    def __init__(self, chain='regtest', loaded=(), existing=(), balance=0):
        self.chain, self.loaded, self.existing, self.balance = chain, set(loaded), set(existing), balance
        self.calls = []

    def __call__(self, *args):
        self.calls.append(args)
        if args[0] == 'getblockchaininfo': return {'chain': self.chain}
        if args[0] == 'listwallets': return list(self.loaded)
        if args[0] == 'listwalletdir': return {'wallets': [{'name': name} for name in self.existing]}
        if args[0] == '-named':
            name = args[2].split('=', 1)[1]
            self.loaded.add(name)
            self.existing.add(name)
            return {'name': name}
        if args[0] == 'generatetoaddress':
            self.balance = 250
            return ['block']
        if args[1] == 'getbalances': return {'mine': {'trusted': self.balance}}
        if args[1] == 'getnewaddress': return 'bcrt1example'
        raise AssertionError(args)


class SeedingTests(unittest.TestCase):
    def test_cli_string_results_are_not_json(self):
        completed = subprocess.CompletedProcess([], 0, stdout='bcrt1address\n', stderr='')
        with patch.object(seed.subprocess, 'run', return_value=completed):
            self.assertEqual(seed.rpc('getnewaddress'), 'bcrt1address')

    def test_non_regtest_is_read_only(self):
        for chain in ('main', 'test', 'signet'):
            rpc = WalletRPC(chain=chain)
            with self.assertRaises(RuntimeError): seed.seed(rpc)
            self.assertEqual(rpc.calls, [('getblockchaininfo',)])

    def test_new_wallets_are_funded_once(self):
        rpc = WalletRPC()
        seed.seed(rpc)
        first = list(rpc.calls)
        self.assertEqual(sum('createwallet' in c for c in first), 2)
        self.assertEqual(sum(c[0] == 'generatetoaddress' for c in first), 1)
        rpc.calls.clear()
        seed.seed(rpc)
        self.assertFalse(any(c[0] in ('-named', 'generatetoaddress') for c in rpc.calls))

    def test_existing_unloaded_wallets_are_loaded_not_recreated(self):
        rpc = WalletRPC(existing=('lez-maker', 'lez-taker'), balance=2)
        seed.seed(rpc)
        self.assertEqual(sum('loadwallet' in c for c in rpc.calls), 2)
        self.assertFalse(any('createwallet' in c or c[0] == 'generatetoaddress' for c in rpc.calls))

    def test_transport_failure_does_not_trigger_creation(self):
        with self.assertRaises(subprocess.CalledProcessError):
            seed.seed(lambda *args: (_ for _ in ()).throw(subprocess.CalledProcessError(1, args)))


class ConfigTests(unittest.TestCase):
    def test_reviewer_volume_namespace_survives_config_regeneration(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root/'market').mkdir()
            env = dict(os.environ, LEZ_MARKET_ROOT=str(root/'market'), LEZ_VOLUME_PREFIX='lez-reviewer-test')
            command = ['bash', str(SCRIPTS/'gen-config.sh'), str(root/'runtime')]
            subprocess.run(command, env=env, check=True, capture_output=True)
            first = (root/'runtime/runtime.env').read_text()
            env.pop('LEZ_VOLUME_PREFIX')
            subprocess.run(command, env=env, check=True, capture_output=True)
            second = (root/'runtime/runtime.env').read_text()
            self.assertIn('LEZ_VOLUME_PREFIX=lez-reviewer-test\n', second)
            self.assertEqual(first, second)
            public = root/'runtime/market-bootstrap.env'
            self.assertEqual(public.read_text(), '')
            manifest = 'M3_POC_LEZ_ESCROW_PROGRAM_ID='+'a'*64+'\n'
            (root/'market/market-bootstrap.env').write_text(manifest)
            (root/'market/market-bootstrap.env').chmod(0o600)
            subprocess.run(command, env=env, check=True, capture_output=True)
            self.assertEqual(public.read_text(), manifest)
            self.assertEqual(public.stat().st_mode & 0o777, 0o644)
            for role in ('maker', 'taker'):
                self.assertEqual((root/'runtime/lez'/role).stat().st_mode & 0o777, 0o755)
            for config in (root/'runtime/config').iterdir():
                self.assertEqual(config.stat().st_mode & 0o777, 0o644)


class RecorderTests(unittest.TestCase):
    def test_another_checkouts_stack_is_rejected_before_capture(self):
        with patch.object(rec, 'command', return_value='/different/checkout/deploy') as command:
            with self.assertRaisesRegex(RuntimeError, 'another checkout'):
                rec.provenance('test-commit')
            self.assertEqual(command.call_count, 1)
            self.assertEqual(command.call_args.args[:2], ('docker', 'inspect'))

    def test_failed_evidence_never_becomes_a_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            runtime = root/'runtime'
            (runtime/'e2e').mkdir(parents=True)
            (runtime/'e2e/concurrent.json').write_text(json.dumps({'result': 'passed'}))
            with patch.object(rec.E, 'RUNTIME', runtime), patch.object(rec.E, 'run', return_value=True), patch.object(rec, 'collect', side_effect=RuntimeError('not finalized')):
                self.assertFalse(rec.record('concurrent', root, 'test-commit'))
            result = json.loads((root/'concurrent/recording-result.json').read_text())
            self.assertFalse(result['passed_including_evidence'])
            self.assertIn('not finalized', (root/'concurrent/execution.log').read_text())
            self.assertEqual(json.loads((root/'concurrent/execution.cast').read_text().splitlines()[0])['version'], 2)

    def test_interrupt_is_retained_as_failure(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(rec.E, 'run', side_effect=KeyboardInterrupt):
            root = pathlib.Path(tmp)
            self.assertFalse(rec.record('taker-refund', root, 'test-commit'))
            self.assertIn('KeyboardInterrupt', (root/'taker-refund/execution.log').read_text())

    def test_output_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root/'concurrent').mkdir()
            with self.assertRaises(FileExistsError): rec.record('concurrent', root, 'test-commit')

    def test_offline_page_escapes_recorded_html_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root/'concurrent').mkdir()
            cast = root/'concurrent/execution.cast'
            cast.write_text(json.dumps({'version': 2})+'\n'+json.dumps([0, 'o', '</script><script>alert(1)</script>'])+'\n')
            rec.finalize(root)
            html = (root/'index.html').read_text()
            self.assertNotIn('</script><script>alert(1)', html)
            sums = (root/'SHA256SUMS').read_text()
            self.assertIn(rec.hashlib.sha256(cast.read_bytes()).hexdigest(), sums)
            self.assertIn('index.html', sums)


if __name__ == '__main__':
    unittest.main()
