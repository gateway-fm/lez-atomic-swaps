"""The public Zcash Testnet lane's preflight, evidence writer and validator
(scripts/zec-public-evidence.py), and node-e2e.py's public mode; no network,
no Docker."""
import email.message
import importlib.util
import io
import json
import os
import pathlib
import re
import sys
import tempfile
import unittest
import urllib.error
from unittest.mock import patch

DEPLOY = pathlib.Path(__file__).resolve().parents[1]
ROOT = DEPLOY.parent
SCRIPTS = DEPLOY / 'scripts'


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    obj = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(obj)
    return obj


lane = module('zec_public_evidence', 'zec-public-evidence.py')
ENV = lane.read_lane_env(DEPLOY / 'zec-public-testnet.env')
TESTNET_GENESIS = ENV['LEZ_ZEC_TESTNET_GENESIS']
TX = {kind: f'{n:064x}' for n, kind in enumerate(('funding', 'claim', 'refund'), start=1)}


class Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class FakeZebra:
    """A JSON-RPC endpoint that answers from a table and records what it was sent."""

    def __init__(self, answers, throttle=0):
        self.answers, self.throttle, self.requests = answers, throttle, []

    def __call__(self, request, timeout=None):
        body = json.loads(request.data)
        self.requests.append((body, dict(request.header_items())))
        if self.throttle:
            self.throttle -= 1
            headers = email.message.Message()
            headers['retry-after'] = '7'
            raise urllib.error.HTTPError(request.full_url, 429, 'Too Many Requests', headers, io.BytesIO(b'{}'))
        answer = self.answers[body['method']]
        result = answer(body['params']) if callable(answer) else answer
        return Response(json.dumps({'jsonrpc': '2.0', 'id': body['id'], 'result': result}).encode())


def chain_info(branch=ENV['LEZ_ZEC_SWAP_PROFILE_BRANCH'], blocks=4_607_100, estimated=4_607_101, chain='test'):
    return {'chain': chain, 'blocks': blocks, 'estimatedheight': estimated, 'verificationprogress': 0.9999998,
            'bestblockhash': '00' * 32, 'consensus': {'chaintip': branch, 'nextblock': branch},
            'upgrades': {'5437f330': {'name': 'NU6.2', 'activationheight': 4_052_000, 'status': 'active'},
                         '77190ad9': {'name': 'NU7', 'activationheight': 4_465_026, 'status': 'active'}}}


def zebra_answers(info=None, genesis=TESTNET_GENESIS):
    return {'getblockchaininfo': info or chain_info(), 'getblockhash': genesis,
            'getnetworkinfo': {'subversion': '/Zebra:7.0.0/', 'version': 7000000, 'connections': 36},
            'getrawtransaction': lambda params: {'height': 4_607_000, 'blockhash': 'ab' * 32, 'confirmations': 12,
                                                 'version': 5, 'versiongroupid': '26a7270a'}}


def route(name='provider', answers=None, throttle=0, credential_file=None):
    fake = FakeZebra(answers or zebra_answers(), throttle)
    kind = 'tatum_testnet_x_api_key' if name == 'provider' else 'self_hosted_cookie'
    url = ENV['LEZ_ZEC_PROVIDER_URL'] if name == 'provider' else ENV['LEZ_ZEC_SELF_HOSTED_URL']
    sleeps = []
    obj = lane.Route(name, url, kind, credential_file, min_interval=0, opener=fake, sleep=sleeps.append)
    return obj, fake, sleeps


def corridor(kind='happy', route_name='provider', status_code=0, confirmations=12, effects=None):
    scenario = 'zec-happy' if kind == 'happy' else 'zec-maker-refund'
    spends = effects or (('funding', 'claim') if kind == 'happy' else ('funding', 'refund'))
    looked = [{'kind': k, 'transaction_id': TX[k], 'reported_height': 4_607_000, 'route': route_name,
               'height': 4_607_000, 'confirmations': confirmations, 'block_hash': 'ab' * 32} for k in spends]
    return lane.corridor_record(route_name, scenario, 'TakerSellsLez', status_code, '2026-11-11T00:00:00Z',
                                '2026-11-11T02:00:00Z', {'swap_id': 'cd' * 32}, looked, [])


def bundle(corridors, preflights=None, artifacts=True):
    with tempfile.TemporaryDirectory() as tmp:
        run = pathlib.Path(tmp)
        (run / 'corridors').mkdir()
        (run / 'provenance.json').write_text(json.dumps({
            'source_sha': '6a78ac0aa4618eb713d68d47b51ce223e1661c9a', 'dirty': False, 'workflow_run': '',
            'artifacts': [{'name': 'lez-maker-node:testnet', 'kind': 'image', 'digest': 'sha256:' + 'e' * 64}] if artifacts else []}))
        for index, preflight in enumerate(preflights if preflights is not None else [lane.preflight(ENV, route()[0])]):
            (run / f'preflight-{index}.json').write_text(json.dumps(preflight))
        for index, record in enumerate(corridors):
            (run / 'corridors' / f'{index}.json').write_text(json.dumps(record))
        (run / 'interventions.jsonl').write_text(json.dumps({'at': '2026-11-11T01:00:00Z', 'what': 'restarted the Taker Node'}) + '\n')
        return lane.assemble(ENV, run)


class LaneProfile(unittest.TestCase):
    def test_the_lane_names_what_the_agreement_profile_pins(self):
        profile = (ROOT / 'crates/zec-swap-sdk/src/profile.rs').read_text()
        block = re.search(r'ZecProfileId::PublicTestnetV1 => Self \{(.*?)\}', profile, re.S).group(1)
        branch = re.search(r'consensus_branch_id: BranchId::(\w+)', block).group(1)
        config = (ROOT / 'crates/zec-reference-actor/src/config.rs').read_text()
        branch_hex = re.search(rf'Self::{branch} => "([0-9a-f]{{8}})"', config).group(1)
        self.assertEqual(ENV['LEZ_ZEC_SWAP_PROFILE'], re.search(r'Self::PublicTestnetV1 => "([^"]+)"', profile).group(1))
        self.assertEqual(ENV['LEZ_ZEC_SWAP_PROFILE_BRANCH'], branch_hex)
        self.assertEqual(ENV['LEZ_ZEC_SWAP_PROFILE_CONFIRMATIONS'], re.search(r'zcash_confirmations: (\d+)', block).group(1))
        self.assertEqual(ENV['LEZ_ZEC_SWAP_PROFILE_REFUND_BLOCKS'], re.search(r'zcash_refund_blocks: (\d+)', block).group(1))

    def test_the_provider_route_is_the_origin_the_actor_pins(self):
        config = (ROOT / 'crates/zec-reference-actor/src/config.rs').read_text()
        pinned = re.search(r'TATUM_TESTNET_ZEBRA_ENDPOINT: &str =\s*"([^"]+)"', config).group(1)
        self.assertEqual(lane.origin(ENV['LEZ_ZEC_PROVIDER_URL']), pinned)


class Preflight(unittest.TestCase):
    def test_a_synced_testnet_on_the_profile_branch_is_supported(self):
        result = lane.preflight(ENV, route()[0])
        self.assertTrue(result['supported'], result['blockers'])
        self.assertEqual(result['zebra']['subversion'], '/Zebra:7.0.0/')
        self.assertEqual(result['endpoint'], 'https://zcash-testnet-zebrad.gateway.tatum.io/')

    def test_a_testnet_on_another_branch_is_blocked_by_name(self):
        result = lane.preflight(ENV, route(answers=zebra_answers(chain_info(branch='77190ad9')))[0])
        self.assertFalse(result['supported'])
        self.assertEqual([c['name'] for c in result['checks'] if not c['ok']], ['consensus_branch'])
        self.assertIn('77190ad9 (NU7)', result['blockers'][0])

    def test_a_regtest_node_on_the_testnet_port_is_refused(self):
        # Zebra reports `test` for Regtest too: only the genesis tells them apart.
        result = lane.preflight(ENV, route('self-hosted', answers=zebra_answers(genesis='02' * 32))[0])
        self.assertEqual([c['name'] for c in result['checks'] if not c['ok']], ['genesis'])

    def test_a_lagging_node_is_not_synchronized(self):
        result = lane.preflight(ENV, route(answers=zebra_answers(chain_info(blocks=4_600_000)))[0])
        self.assertEqual([c['name'] for c in result['checks'] if not c['ok']], ['synchronized'])

    def test_a_rate_limit_is_waited_out_and_recorded(self):
        obj, fake, sleeps = route(throttle=1)
        lane.preflight(ENV, obj)
        self.assertIn(7.0, sleeps)
        self.assertEqual(obj.provider_errors[0]['http_status'], 429)

    def test_it_never_broadcasts_or_mines(self):
        obj = route()[0]
        for method in ('sendrawtransaction', 'generate', 'submitblock'):
            with self.assertRaises(lane.LaneError):
                obj.call(method, ['00'])

    def test_the_key_goes_into_one_header_and_nowhere_else(self):
        with tempfile.NamedTemporaryFile('w', suffix='.key') as key:
            key.write('tatum-test-key-0123456789abcdef\n'); key.flush()
            obj, fake, _ = route(credential_file=key.name)
            result = lane.preflight(ENV, obj)
            self.assertEqual({h.get('X-api-key') for _, h in fake.requests}, {'tatum-test-key-0123456789abcdef'})
            self.assertNotIn('tatum-test-key', json.dumps(result))
            self.assertEqual(result['credential'], 'file')

    def test_a_cookie_is_basic_auth(self):
        with tempfile.NamedTemporaryFile('w') as cookie:
            cookie.write('__cookie__:0123456789abcdefcookie'); cookie.flush()
            obj, fake, _ = route('self-hosted', credential_file=cookie.name)
            obj.call('getblockhash', [0])
            self.assertTrue(fake.requests[0][1]['Authorization'].startswith('Basic '))

    def test_endpoints_carry_no_credentials(self):
        with self.assertRaises(lane.LaneError):
            lane.origin('https://zcash-testnet-zebrad.gateway.tatum.io/?apikey=x')
        with self.assertRaises(lane.LaneError):
            lane.route_from_env({**ENV, 'LEZ_ZEC_SELF_HOSTED_URL': 'http://10.0.0.5:18232/'}, 'self-hosted')


class Corridors(unittest.TestCase):
    def test_the_zcash_effects_of_a_summary(self):
        summary = {'swap_id': 'x', 'zcash_funding': {'transaction_id': TX['funding'], 'height': 5},
                   'zcash_claim': {'transaction_id': TX['claim'], 'height': 7}, 'zcash_refund_height': 200}
        self.assertEqual([e['kind'] for e in lane.zcash_effects(summary)], ['claim', 'funding'])

    def test_a_passed_harness_whose_transactions_are_not_on_the_route_fails(self):
        record = lane.corridor_record('provider', 'zec-happy', 'TakerSellsLez', 0, 'a', 'b', {}, [
            {'kind': 'funding', 'transaction_id': TX['funding'], 'height': None}], [])
        self.assertEqual(record['status'], 'failed')
        self.assertIn('not all found', record['reason'])

    def test_exit_three_is_blocked(self):
        record = lane.corridor_record('provider', 'zec-happy', 'TakerSellsLez', 3, 'a', 'b', None, None, None,
                                      'the provisioner is Regtest-only')
        self.assertEqual((record['status'], record['kind']), ('blocked', 'happy'))


class Bundle(unittest.TestCase):
    def test_a_full_lane_is_valid_and_complete(self):
        evidence = bundle([corridor('happy'), corridor('refund', 'self-hosted')])
        self.assertEqual(lane.validate(evidence), [])
        self.assertEqual((evidence['status'], evidence['evidence_kind']), ('passed', 'public-chain-swap'))
        self.assertTrue(evidence['coverage']['acceptance_complete'])
        self.assertEqual(evidence['interventions'][0]['what'], 'restarted the Taker Node')

    def test_one_route_is_not_acceptance(self):
        evidence = bundle([corridor('happy'), corridor('refund')])
        self.assertEqual(evidence['status'], 'passed')
        self.assertFalse(evidence['coverage']['acceptance_complete'])

    def test_a_refund_without_its_refund_transaction_is_invalid(self):
        evidence = bundle([corridor('refund', effects=('funding',))])
        self.assertTrue(any('refund' in p for p in lane.validate(evidence)))

    def test_too_few_confirmations_are_invalid(self):
        evidence = bundle([corridor('happy', confirmations=3)])
        self.assertTrue(any('the profile wants 10' in p for p in lane.validate(evidence)))

    def test_passed_needs_artifact_digests(self):
        evidence = bundle([corridor('happy')], artifacts=False)
        self.assertIn('status passed without the digests of the artifacts that ran', lane.validate(evidence))

    def test_a_blocked_preflight_is_never_swap_evidence(self):
        blocked = lane.preflight(ENV, route(answers=zebra_answers(chain_info(branch='77190ad9')))[0])
        evidence = bundle([], [blocked])
        self.assertEqual((evidence['status'], evidence['evidence_kind']), ('blocked', 'public-chain-preflight'))
        self.assertEqual(lane.validate(evidence), [])
        self.assertIn('No swap transaction in this bundle', lane.summary_markdown(evidence))

    def test_a_preflight_only_run(self):
        self.assertEqual(bundle([])['status'], 'preflight')

    def test_a_regtest_route_proves_nothing(self):
        regtest = lane.preflight(ENV, route('self-hosted', answers=zebra_answers(genesis='02' * 32))[0])
        self.assertEqual(bundle([], [regtest])['evidence_kind'], 'none')

    def test_a_field_named_like_a_secret_is_refused(self):
        evidence = bundle([corridor('happy')])
        evidence['corridors'][0]['taker_private_key'] = 'redacted'
        self.assertTrue(any('named like a secret' in p for p in lane.validate(evidence)))

    def test_secret_file_contents_are_never_written(self):
        evidence = bundle([corridor('happy')])
        with tempfile.TemporaryDirectory() as tmp, tempfile.NamedTemporaryFile('w') as key:
            key.write('tatum-test-key-0123456789abcdef'); key.flush()
            evidence['interventions'].append({'at': 'now', 'what': 'pasted tatum-test-key-0123456789abcdef by mistake'})
            problems = lane.write_bundle(evidence, pathlib.Path(tmp) / 'b', [key.name])
            self.assertTrue(any('secret file' in p for p in problems))
            self.assertFalse((pathlib.Path(tmp) / 'b' / 'evidence.json').exists())

    def test_the_written_bundle_validates_from_disk(self):
        evidence = bundle([corridor('happy'), corridor('refund', 'self-hosted')])
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp) / 'bundle'
            self.assertEqual(lane.write_bundle(evidence, out, []), [])
            sums = dict(reversed(line.split('  ')) for line in (out / 'SHA256SUMS').read_text().splitlines())
            self.assertEqual(set(sums), {'evidence.json', 'SUMMARY.md'})
            with patch.object(sys, 'stdout', io.StringIO()):
                self.assertEqual(lane.main(['validate', str(out)]), 0)


class NodeE2EPublicMode(unittest.TestCase):
    def load(self):
        with patch.dict(os.environ, {'LEZ_ZEC_PUBLIC_TESTNET': '1', 'LEZ_ZEC_TIMEOUT_SCALE': '12',
                                     'LEZ_ZEC_BLOCK_WAIT_SECONDS': '60'}):
            return module('node_e2e_public', 'node-e2e.py')

    def test_it_waits_for_testnet_blocks_and_never_mines(self):
        e2e = self.load()
        calls, heights = [], iter([100, 100, 101, 102, 102])
        def fake(method, params=None):
            calls.append(method)
            return next(heights)
        with patch.object(e2e, 'zebra', fake), patch.object(e2e.time, 'sleep'), patch.object(e2e, 'log'):
            e2e.zebra_mine(2, 'refund height')
            e2e.zec_mine_pending()
            e2e.zec_mining_policy('both_legs_locked', {})
        self.assertNotIn('generate', calls)
        self.assertEqual(e2e.zec_timeout(2400), 28_800)

    def test_the_devnet_still_mines(self):
        e2e = module('node_e2e_devnet', 'node-e2e.py')
        calls = []
        with patch.object(e2e, 'zebra', lambda method, params=None: calls.append(method) or 1), patch.object(e2e, 'log'):
            e2e.zebra_mine(1, 'mempool')
        self.assertEqual(calls[0], 'generate')
        self.assertEqual(e2e.zec_timeout(2400), 2400)


if __name__ == '__main__':
    unittest.main()
