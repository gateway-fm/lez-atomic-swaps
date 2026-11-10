"""Chain lifecycle contracts of the harness scripts (#113, #103 item 5).

down.sh --wipe retires the wiped chain's escrow deployment, market-bootstrap.sh
reuses a deployment only on the chain it was made on, and reset-swaps.sh will
not forget an open swap unless told to. Docker, curl and the deployer are
replaced by fakes on PATH; nothing here needs a stack.
"""
import json
import os
import pathlib
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'
LIVE = 'a' * 64
OLD = 'b' * 64
PROGRAM = 'c22d61fc00d68083a01bc20c607423e015b9aeb862bbcdf7681b07848368221b'
WALLETS = ('maker-munich-01', 'maker-basel-02', 'taker-zurich-01', 'taker-limmat-02')


def executable(path, text):
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class Workspace(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bin = self.tmp / 'bin'
        self.bin.mkdir()
        self.deploy = self.tmp / 'deploy'
        (self.deploy / 'scripts').mkdir(parents=True)
        self.market = self.tmp / 'market'
        (self.market / 'bootstrap').mkdir(parents=True)
        self.calls = self.tmp / 'calls.log'

    def env(self, **extra):
        return {**os.environ, 'PATH': f'{self.bin}:{os.environ["PATH"]}', 'CALLS': str(self.calls), **extra}

    def logged(self):
        return self.calls.read_text() if self.calls.exists() else ''


class DownWipe(Workspace):
    def setUp(self):
        super().setUp()
        shutil.copy(SCRIPTS / 'down.sh', self.deploy / 'scripts')
        executable(self.bin / 'docker', '#!/bin/sh\necho "docker $*" >>"$CALLS"\n')
        (self.deploy / 'runtime').mkdir()
        (self.deploy / 'runtime' / 'runtime.env').write_text(f'LEZ_MARKET_ROOT={self.market}\n')
        (self.market / 'identities' / 'maker-munich-01').mkdir(parents=True)
        (self.market / 'identities' / 'maker-munich-01' / 'identity.json').write_text('{}')
        (self.market / 'bootstrap' / 'deployment.json').write_text('{}')
        (self.market / 'market-bootstrap.env').write_text('M3_POC_LEZ_GENESIS_BLOCK_HASH=' + OLD + '\n')

    def down(self, *args):
        return subprocess.run(['bash', str(self.deploy / 'scripts' / 'down.sh'), *args],
                              env=self.env(), capture_output=True, text=True, check=True)

    def test_wipe_retires_the_chains_deployment_and_keeps_identities(self):
        self.down('--wipe')
        self.assertFalse((self.deploy / 'runtime').exists())
        self.assertFalse((self.market / 'bootstrap' / 'deployment.json').exists())
        self.assertFalse((self.market / 'market-bootstrap.env').exists())
        self.assertEqual(len(list((self.market / 'bootstrap').glob('deployment.json.chain-*.bak'))), 1)
        self.assertEqual(len(list(self.market.glob('market-bootstrap.env.chain-*.bak'))), 1)
        self.assertTrue((self.market / 'identities' / 'maker-munich-01' / 'identity.json').exists())
        self.assertIn('down -v', self.logged())

    def test_plain_down_keeps_everything(self):
        self.down()
        self.assertTrue((self.deploy / 'runtime' / 'runtime.env').exists())
        self.assertTrue((self.market / 'bootstrap' / 'deployment.json').exists())
        self.assertTrue((self.market / 'market-bootstrap.env').exists())


# The indexer: getBlockById 1 answers the live genesis; every vault reads 0 and
# every owner 100, so the claims count as done and only the deployment matters.
FAKE_CURL = '''#!/usr/bin/env python3
import json, os, sys
body = json.loads(sys.argv[sys.argv.index("-d") + 1])
if body["method"] == "getBlockById":
    print(json.dumps({"result": {"header": {"block_id": 1, "hash": os.environ["FAKE_GENESIS"]}}}))
elif body["method"] == "getAccount":
    print(json.dumps({"result": {"balance": 0 if body["params"][0].startswith("vault") else 100}}))
else:
    sys.exit(22)
'''

FAKE_DEPLOYER = '''#!/bin/sh
echo deployed >>"$CALLS"
cat <<EOF
{"transaction_hash":"''' + 'd' * 64 + '''","inclusion_block_id":12,
 "preflight":{"image_id":"$ESCROW_PROGRAM_ID","genesis_block_hash":"$FAKE_DEPLOY_GENESIS","last_block_id":8}}
EOF
'''


class MarketBootstrapGenesis(Workspace):
    def setUp(self):
        super().setUp()
        executable(self.bin / 'curl', FAKE_CURL)
        executable(self.bin / 'deployer', FAKE_DEPLOYER)
        for wallet in WALLETS:
            (self.market / 'identities' / wallet).mkdir(parents=True)
            (self.market / 'identities' / wallet / 'identity.json').write_text(
                json.dumps({'account_id': f'owner-{wallet}', 'vault_account_id': f'vault-{wallet}'}))

    def evidence(self, genesis):
        (self.market / 'bootstrap' / 'deployment.json').write_text(json.dumps({
            'transaction_hash': 'e' * 64, 'inclusion_block_id': 9,
            'preflight': {'image_id': PROGRAM, 'genesis_block_hash': genesis, 'last_block_id': 8}}))
        (self.market / 'market-bootstrap.env').write_text(f'M3_POC_LEZ_GENESIS_BLOCK_HASH={genesis}\n')

    def bootstrap(self, deploy_genesis=LIVE):
        return subprocess.run(['bash', str(SCRIPTS / 'market-bootstrap.sh')], capture_output=True, text=True,
                              env=self.env(MARKET_ROOT=str(self.market), ESCROW_PROGRAM_ID=PROGRAM,
                                           DEPLOYER=str(self.bin / 'deployer'), FAKE_GENESIS=LIVE,
                                           FAKE_DEPLOY_GENESIS=deploy_genesis,
                                           LEZ_WALLET_MAKER_ALLOCATION='100', LEZ_WALLET_TAKER_ALLOCATION='100'))

    def manifest_genesis(self):
        for line in (self.market / 'market-bootstrap.env').read_text().splitlines():
            if line.startswith('M3_POC_LEZ_GENESIS_BLOCK_HASH='):
                return line.split('=', 1)[1]
        return None

    def test_deployment_on_this_chain_is_reused(self):
        self.evidence(LIVE)
        run = self.bootstrap()
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertIn('escrow program already deployed', run.stdout)
        self.assertNotIn('deployed', self.logged())
        self.assertEqual(self.manifest_genesis(), LIVE)

    def test_deployment_on_another_chain_is_retired_and_redone(self):
        self.evidence(OLD)
        run = self.bootstrap()
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertIn('is for another chain', run.stdout)
        self.assertIn('deployed', self.logged())
        self.assertEqual(len(list((self.market / 'bootstrap').glob('deployment.json.chain-*.bak'))), 1)
        self.assertEqual(len(list(self.market.glob('market-bootstrap.env.chain-*.bak'))), 1)
        recorded = json.loads((self.market / 'bootstrap' / 'deployment.json').read_text())
        self.assertEqual(recorded['preflight']['genesis_block_hash'], LIVE)
        self.assertEqual(self.manifest_genesis(), LIVE)

    def test_a_deployment_recorded_for_another_genesis_is_refused(self):
        run = self.bootstrap(deploy_genesis=OLD)
        self.assertEqual(run.returncode, 2)
        self.assertIn('deployment evidence invalid', run.stderr)
        self.assertFalse((self.market / 'bootstrap' / 'deployment.json').exists())
        self.assertFalse((self.market / 'market-bootstrap.env').exists())


# docker: both role containers run; the Taker's owner call answers FAKE_TAKER_LIST
# (empty: no answer), the Maker's store read prints FAKE_MAKER_OPEN. Every other
# call is logged and succeeds.
FAKE_DOCKER = '''#!/bin/sh
case "$1 $2" in
  "inspect -f") echo "${FAKE_RUNNING:-true}"; exit 0 ;;
  "exec lez-taker-node") case "$3" in curl) [ -n "$FAKE_TAKER_LIST" ] || exit 7; echo "$FAKE_TAKER_LIST"; exit 0 ;; esac ;;
  "exec lez-maker-node") case "$3" in python3) printf '%s' "$FAKE_MAKER_OPEN"; exit 0 ;; esac ;;
esac
echo "docker $*" >>"$CALLS"
'''


def taker_list(*states):
    return json.dumps({'result': {'swaps': [{'swap_id': f'swap-{i}', 'state': s} for i, s in enumerate(states)]}})


class ResetSwapsRefusesOpenSwaps(Workspace):
    def setUp(self):
        super().setUp()
        shutil.copy(SCRIPTS / 'reset-swaps.sh', self.deploy / 'scripts')
        executable(self.bin / 'docker', FAKE_DOCKER)
        (self.deploy / 'runtime').mkdir()
        (self.deploy / 'runtime' / 'runtime.env').write_text('BTC_RPC_PASSWORD=test\n')

    def reset(self, *args, **fake):
        return subprocess.run(['bash', str(self.deploy / 'scripts' / 'reset-swaps.sh'), *args],
                              env=self.env(**fake), capture_output=True, text=True)

    def assert_untouched(self):
        # neither a hold, a stop nor a store write: only the reads above ran
        self.assertEqual(self.logged(), '')

    def test_an_open_taker_swap_is_refused(self):
        run = self.reset(FAKE_TAKER_LIST=taker_list('completed', 'claim_in_progress'), FAKE_MAKER_OPEN='')
        self.assertEqual(run.returncode, 1)
        self.assertIn('taker swap-1 claim_in_progress', run.stderr)
        self.assertNotIn('swap-0', run.stderr)
        self.assertIn('--abandon-open', run.stderr)
        self.assert_untouched()

    def test_an_open_maker_actor_is_refused(self):
        run = self.reset(FAKE_TAKER_LIST=taker_list('refunded'), FAKE_MAKER_OPEN='maker swap-9 backoff\n')
        self.assertEqual(run.returncode, 1)
        self.assertIn('maker swap-9 backoff', run.stderr)
        self.assert_untouched()

    def test_a_taker_that_cannot_be_asked_is_refused(self):
        run = self.reset(FAKE_TAKER_LIST='', FAKE_MAKER_OPEN='')
        self.assertEqual(run.returncode, 1)
        self.assertIn('taker - unknown', run.stderr)
        self.assert_untouched()

    def test_abandon_open_proceeds_and_archives(self):
        run = self.reset('--abandon-open', FAKE_TAKER_LIST=taker_list('claim_in_progress'), FAKE_MAKER_OPEN='')
        self.assertIn('abandoning the open swaps below', run.stdout)
        # the role steps receive the archive stamp
        self.assertRegex(self.logged(), r'_ /var/lib/lez/taker \d{8}T\d{6}Z')
        self.assertRegex(self.logged(), r'_ /var/lib/lez/maker \d{8}T\d{6}Z')

    def test_settled_swaps_are_forgotten_without_an_archive(self):
        self.reset(FAKE_TAKER_LIST=taker_list('completed', 'refunded'), FAKE_MAKER_OPEN='')
        self.assertIn('_ /var/lib/lez/taker \n', self.logged() + '\n')
        self.assertNotRegex(self.logged(), r'\d{8}T\d{6}Z')

    def test_unknown_argument(self):
        self.assertEqual(self.reset('--bogus').returncode, 64)
        self.assert_untouched()


if __name__ == '__main__':
    unittest.main()
