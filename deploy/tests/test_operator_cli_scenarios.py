"""The scenarios that drive a Node through its operator CLI, the outage's
publish and the refund path's Zcash kill points: what can be checked without a
stack. Docker and the Nodes are never called; node-e2e.py is loaded and its
helpers are driven with fakes, and the Rust strings the scenarios assert on are
read from the sources.
"""
import importlib.util
import json
import os
import pathlib
import subprocess
import unittest
from unittest.mock import patch

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'
REPO = SCRIPTS.parents[1]


def load():
    clean = {k: v for k, v in os.environ.items() if not k.startswith('LEZ_')}
    with patch.dict(os.environ, clean, clear=True):
        spec = importlib.util.spec_from_file_location('node_e2e_operator_cli', SCRIPTS / 'node-e2e.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return module


e2e = load()
e2e.log = lambda message: None
BTC = {'pair': 'Bitcoin', 'direction': 'TakerSellsForeign'}
SWAP = 'ab' * 32


def done(stdout='', stderr='', code=0):
    return subprocess.CompletedProcess([], code, stdout, stderr)


class MakerCliMapping(unittest.TestCase):
    """MAKER_CLI: every Maker owner call of a wrapped scenario is one
    lez-maker-cli command, and a call the CLI cannot make fails the scenario."""

    def test_each_owner_method_is_its_cli_command(self):
        route = {'pair': 'Zcash', 'direction': 'TakerSellsLez'}
        cases = {
            ('maker_pair_list', '{}'): ['pairs'],
            ('maker_local_price_list', '{}'): ['prices'],
            ('maker_health', '{}'): ['health'],
            ('maker_offer_list', '{}'): ['offers'],
            ('swap_history', '{}'): ['history'],
            ('maker_wallet_balances_v1', '{}'): ['balances'],
            ('maker_wallet_balances_v1', '{"zcash": true}'): ['balances', '--zcash'],
            ('swap_status', json.dumps({'id': SWAP})): ['status', '--id', SWAP],
            ('maker_actor_monitor_v1', json.dumps({'id': SWAP})): ['monitor', '--id', SWAP],
            ('maker_price_quote', json.dumps({'route': route})):
                ['quote', '--pair', 'zcash', '--direction', 'taker-sells-lez'],
            ('maker_offer_publish', json.dumps({'request_id': 'r-1', 'offer_id': 'offer-0001', 'route': BTC})):
                ['publish-offer', '--request-id', 'r-1', '--offer-id', 'offer-0001', '--pair', 'bitcoin',
                 '--direction', 'taker-sells-foreign'],
            ('maker_offer_withdraw', json.dumps({'request_id': 'r-2', 'offer_id': 'offer-0001', 'expected_revision': 3})):
                ['withdraw-offer', '--request-id', 'r-2', '--offer-id', 'offer-0001', '--expected-revision', '3'],
            ('maker_actor_refund_v1', json.dumps({'request_id': 'r-3', 'id': SWAP, 'expected_generation': 7})):
                ['refund', '--id', SWAP, '--request-id', 'r-3', '--expected-generation', '7'],
            ('maker_actor_claim_v1', json.dumps({'request_id': 'r-4', 'id': SWAP, 'expected_generation': 8})):
                ['claim', '--id', SWAP, '--request-id', 'r-4', '--expected-generation', '8'],
        }
        for (method, params), command in cases.items():
            self.assertEqual(e2e.maker_cli_arguments(method, json.loads(params)), command, method)

    def test_every_mapped_command_is_one_the_cli_has(self):
        source = (REPO / 'crates/maker-node/src/bin/lez-maker-cli.rs').read_text()
        for command, method in (('Pairs', 'maker_pair_list'), ('Prices', 'maker_local_price_list'),
                                ('Health', 'maker_health'), ('Offers', 'maker_offer_list'), ('History', 'swap_history'),
                                ('Balances', 'maker_wallet_balances_v1'), ('Status', 'swap_status'),
                                ('Monitor', 'maker_actor_monitor_v1'), ('Quote', 'maker_price_quote'),
                                ('PublishOffer', 'maker_offer_publish'), ('WithdrawOffer', 'maker_offer_withdraw'),
                                ('Claim', 'maker_actor_claim_v1'), ('Refund', 'maker_actor_refund_v1')):
            self.assertRegex(source, rf'\n    {command}\b', command)
            self.assertIn(f'"{method}"', source)
        # the one method the wrapped scenarios use that the CLI has no command for
        self.assertNotIn('maker_local_route_save_v1', source)

    def test_a_method_without_a_command_fails_the_scenario(self):
        with self.assertRaisesRegex(e2e.Failure, 'lez-maker-cli has no command for maker_actor_repin_v1'):
            e2e.maker_cli_arguments('maker_actor_repin_v1', {'id': SWAP})
        # `offers` lists every offer: a filtered list is not something the CLI asks for
        with self.assertRaisesRegex(e2e.Failure, 'no command for maker_offer_list with'):
            e2e.maker_cli_arguments('maker_offer_list', {'states': ['active']})

    def test_the_switch_sends_a_maker_call_to_the_cli_and_is_put_back(self):
        ran = []

        def run(command, **_):
            ran.append(command)
            return done('[{"id": "x"}]')

        def scenario(stamp):
            self.assertTrue(e2e.MAKER_CLI)
            return {'history': e2e.call('maker', 'swap_history', {})}

        with patch.object(e2e.subprocess, 'run', run):
            self.assertEqual(e2e.through_maker_cli(scenario)('1700000000'), {'history': [{'id': 'x'}]})
        self.assertEqual(ran, [['docker', 'exec', e2e.NODES['maker'][0], 'lez-maker-cli', 'history']])
        self.assertFalse(e2e.MAKER_CLI)
        with patch.object(e2e.subprocess, 'run', lambda *a, **k: self.fail('nothing may run')):
            with self.assertRaises(e2e.Failure):
                e2e.through_maker_cli(lambda stamp: e2e.call('maker', 'maker_actor_repin_v1', {'id': SWAP}))('1')
        self.assertFalse(e2e.MAKER_CLI)

    def test_the_cli_error_is_the_rpc_error_it_printed(self):
        stale = done(stderr='Error: local RPC error -32009: maker actor manual action generation is stale\n', code=1)
        self.assertEqual(e2e.cli_reply('lez-maker-cli', stale),
                         {'error': {'code': -32009, 'message': 'maker actor manual action generation is stale'}})
        self.assertIn(e2e.STALE_GENERATION, e2e.cli_reply('lez-maker-cli', stale)['error']['message'])
        refused = done(stderr='local RPC error -32010: Taker dependency unavailable '
                              '{"category":"authenticated_delivery_unavailable"}\n', code=2)
        self.assertEqual(e2e.cli_reply('lez-taker-cli', refused), {'error': {
            'code': -32010, 'message': 'Taker dependency unavailable',
            'data': {'category': 'authenticated_delivery_unavailable'}}})
        # no socket, a stopped container, the CLI's own timeout: a transport failure, as rpc() names it
        down = done(stderr='Error: connect local RPC socket /run/lez/maker/node.sock\n\nCaused by:\n    No such file\n', code=1)
        self.assertEqual(e2e.cli_reply('lez-maker-cli', down)['error']['code'], -1)
        self.assertEqual(e2e.cli_reply('lez-maker-cli', done('{"revision":2,"was_replay":false}\n')),
                         {'result': {'revision': 2, 'was_replay': False}})

    def test_a_stopped_maker_reads_unreachable_through_the_cli(self):
        with patch.object(e2e, 'MAKER_CLI', True), \
                patch.object(e2e.subprocess, 'run', lambda *a, **k: done(stderr='Error response from daemon: not running', code=1)):
            self.assertEqual(e2e.maker_phase(SWAP), 'unreachable')

    def test_the_route_is_saved_disabled_priced_then_enabled(self):
        ran = []
        lists = {'pairs': [{'revision': 4, 'value': {'route': BTC}}], 'prices': []}

        def run(command, **_):
            arguments = command[4:]
            ran.append(arguments)
            if arguments[0] in lists:
                return done(json.dumps(lists[arguments[0]]))
            if arguments[0] == 'configure-pair':
                lists['pairs'][0]['revision'] += 1
            return done(json.dumps({'revision': lists['pairs'][0]['revision'], 'was_replay': False}))

        request = {'request_id': 'e2e-route-17', 'expected_pair_revision': 4,
                   'configuration': {'route': BTC, 'enabled': True, 'price_source': 'local', 'minimum_foreign_units': 1_000_000,
                                     'maximum_foreign_units': 1_000_000, 'offer_ttl_seconds': 3600},
                   'price': {'route': BTC, 'lez_units_per_lot': 1, 'foreign_units_per_lot': 1000}}
        with patch.object(e2e.subprocess, 'run', run):
            reply = e2e.maker_cli_rpc('maker_local_route_save_v1', request)
        self.assertIn('result', reply)
        route = ['--pair', 'bitcoin', '--direction', 'taker-sells-foreign']
        policy = ['--price-source', 'local', '--minimum-foreign-units', '1000000', '--maximum-foreign-units', '1000000',
                  '--offer-ttl-seconds', '3600']
        self.assertEqual(ran, [
            ['pairs'],
            ['configure-pair', '--request-id', 'e2e-route-17-pair-off', '--expected-revision', '4', *route,
             '--enabled', 'false', *policy],
            ['prices'],
            # no price yet: created, so no --expected-revision
            ['set-local-price', '--request-id', 'e2e-route-17-price', *route, '--lez-units-per-lot', '1',
             '--foreign-units-per-lot', '1000'],
            ['pairs'],
            ['configure-pair', '--request-id', 'e2e-route-17-pair-on', '--expected-revision', '5', *route,
             '--enabled', 'true', *policy]])

    def test_a_refused_step_is_the_answer_and_nothing_follows(self):
        ran = []

        def run(command, **_):
            ran.append(command[4])
            if command[4] == 'pairs':
                return done('[]')
            return done(stderr='Error: local RPC error -32009: maker configuration revision is stale\n', code=1)

        request = {'request_id': 'r', 'configuration': {'route': BTC, 'enabled': True, 'price_source': 'local',
                                                        'minimum_foreign_units': 1, 'maximum_foreign_units': 1,
                                                        'offer_ttl_seconds': 60},
                   'price': {'route': BTC, 'lez_units_per_lot': 1, 'foreign_units_per_lot': 1}}
        with patch.object(e2e.subprocess, 'run', run):
            self.assertEqual(e2e.maker_cli_rpc('maker_local_route_save_v1', request)['error']['code'], -32009)
        self.assertEqual(ran, ['pairs', 'configure-pair'])


class TakerCliMapping(unittest.TestCase):
    def test_balances_ask_for_the_zcash_and_monero_wallets_by_flag(self):
        self.assertEqual(e2e.taker_cli_arguments('taker_wallet_balances_v1', {'schema_version': 1}), ['balances'])
        self.assertEqual(e2e.taker_cli_arguments('taker_wallet_balances_v1', {'schema_version': 1, 'monero': True}),
                         ['balances', '--monero'])
        self.assertEqual(e2e.taker_cli_arguments('taker_wallet_balances_v1', {'schema_version': 1, 'zcash': True}),
                         ['balances', '--zcash'])
        source = (REPO / 'crates/taker-node/src/bin/lez-taker-cli.rs').read_text()
        self.assertIn('balances_request(zcash, monero)', source)

    def test_a_monero_take_and_claim_are_cli_commands(self):
        route = {'pair': 'Monero', 'direction': 'TakerSellsLez'}
        self.assertEqual(e2e.taker_cli_arguments('taker_offer_list_v1', {'schema_version': 1, 'route': route}),
                         ['swap', 'offers', '--pair', 'monero', '--direction', 'taker-sells-lez'])
        self.assertEqual(e2e.taker_cli_arguments('taker_swap_initiate_v1', {
            'request_id': 'e2e-take-1', 'offer_id': 'offer-e2e-xmr-1', 'foreign_units': e2e.XMR_FOREIGN_UNITS}),
            ['swap', 'take', '--request-id', 'e2e-take-1', '--offer-id', 'offer-e2e-xmr-1', '--foreign-units',
             '1000000000000'])
        self.assertEqual(e2e.taker_cli_arguments('taker_swap_refund_v1', {
            'request_id': 'e2e-refund-1', 'swap_id': SWAP, 'expected_generation': 5}),
            ['swap', 'refund', '--swap-id', SWAP, '--request-id', 'e2e-refund-1', '--expected-generation', '5'])
        with self.assertRaisesRegex(e2e.Failure, 'lez-taker-cli has no command for taker_swap_export_v1'):
            e2e.taker_cli_arguments('taker_swap_export_v1', {})

    def test_the_wrapped_scenarios_are_registered_through_the_cli(self):
        for table, name in ((e2e.XMR_SCENARIOS, 'xmr-taker-cli'), (e2e.ZEC_SCENARIOS, 'zec-taker-cli-refund')):
            self.assertIn(name, table)
        seen = []
        with patch.object(e2e, 'scenario_xmr_happy', lambda stamp: seen.append(e2e.TAKER_CLI) or {'swap_id': SWAP}):
            self.assertEqual(e2e.through_taker_cli(e2e.scenario_xmr_happy)('1'), {'swap_id': SWAP})
        self.assertEqual(seen, [True])
        self.assertFalse(e2e.TAKER_CLI)


class OwnerReads(unittest.TestCase):
    MAKER = {'schema_version': 1, 'ready': True, 'degraded': False, 'delivery': 'available', 'chat': 'available',
             'routes': [{'route': BTC, 'state': 'available'}]}
    TAKER = {'schema_version': 1, 'ready': True, 'degraded': False, 'delivery': 'available', 'chat': 'available',
             'pair_capabilities': [{'pair': 'Bitcoin', 'supported_direction': 'TakerSellsForeign'}],
             'registered_methods': {name: True for name in ('health', 'offer_list', 'swap_list', 'initiate', 'monitor',
                                                            'claim', 'refund', 'lock')}}
    BALANCES = {'schema_version': 1,
                'bitcoin': {'state': 'available', 'wallet': 'lez-maker', 'network': 'regtest', 'trusted_sat': 5,
                            'untrusted_pending_sat': 0, 'immature_sat': 0},
                'lez': {'state': 'available', 'owner_account_hex': 'aa', 'owner_account_base58': 'Acc1',
                        'balance_atomic_units': 990, 'nonce': 4}}

    def test_health_reads_as_documented_for_both_roles(self):
        with patch.object(e2e, 'ROUTE', dict(BTC)):
            self.assertIs(e2e.assert_cli_health('maker', self.MAKER, degraded=False), self.MAKER)
            self.assertIs(e2e.assert_cli_health('taker', self.TAKER, degraded=False), self.TAKER)
            cut = {**self.MAKER, 'degraded': True, 'delivery': 'unavailable', 'chat': 'unavailable'}
            e2e.assert_cli_health('maker', cut, degraded=True)

    def test_health_that_differs_fails_naming_the_field(self):
        with patch.object(e2e, 'ROUTE', dict(BTC)):
            for role, reported, named in (
                    ('maker', {**self.MAKER, 'degraded': True}, 'degraded True'),
                    ('maker', {**self.MAKER, 'chat': 'unavailable'}, "chat 'unavailable'"),
                    ('maker', {**self.MAKER, 'ready': 1}, 'ready 1'),
                    ('maker', {**self.MAKER, 'routes': []}, 'routes names'),
                    ('maker', {**self.MAKER, 'routes': [{'route': BTC, 'state': 'unavailable'}]}, 'routes names'),
                    ('taker', {**self.TAKER, 'registered_methods': {}}, 'registered_methods lacks'),
                    ('taker', {**self.TAKER, 'pair_capabilities': []}, 'pair_capabilities has no row')):
                with self.assertRaisesRegex(e2e.Failure, f'lez-{role}-cli health is not as documented: .*{named}'):
                    e2e.assert_cli_health(role, reported, degraded=False)
            # under the cut an available dependency is the failure
            with self.assertRaisesRegex(e2e.Failure, "delivery 'available', expected 'unavailable'"):
                e2e.assert_cli_health('maker', {**self.MAKER, 'degraded': True}, degraded=True)

    def test_the_fields_health_names_are_the_nodes(self):
        maker = (REPO / 'crates/maker-node/src/rpc_contracts.rs').read_text()
        taker = (REPO / 'crates/taker-node/src/taker_facade.rs').read_text()
        for field in ('schema_version', 'ready', 'degraded', 'delivery', 'chat', 'routes'):
            self.assertRegex(maker, rf'\n    {field}: ')
        for field in ('pair_capabilities', 'registered_methods', 'supported_direction', 'offer_list', 'swap_list',
                      'initiate', 'lock'):
            self.assertRegex(taker, rf'\n    {field}: ')

    def test_balances_must_be_whole_numbers_and_the_indexers_lez(self):
        with patch.object(e2e, 'lez_account_balance', lambda account: {'Acc1': 990}[account]):
            self.assertEqual(e2e.assert_cli_balances('maker', lambda: self.BALANCES), self.BALANCES)
        broken = json.loads(json.dumps(self.BALANCES))
        broken['lez']['balance_atomic_units'] = '990'
        with patch.object(e2e, 'lez_account_balance', lambda account: self.fail('not reached')):
            with self.assertRaisesRegex(e2e.Failure, "balance_atomic_units '990' is not a whole number"):
                e2e.assert_cli_balances('taker', lambda: broken)
        unnamed = json.loads(json.dumps(self.BALANCES))
        unnamed['bitcoin']['wallet'] = None
        with self.assertRaisesRegex(e2e.Failure, 'an available Core wallet without a name'):
            e2e.assert_cli_balances('taker', lambda: unnamed)

    def test_a_block_between_the_two_reads_is_read_again_and_a_difference_fails(self):
        indexed = iter([1000, 990])
        with patch.object(e2e, 'lez_account_balance', lambda account: next(indexed)), \
                patch.object(e2e.time, 'sleep', lambda seconds: None):
            e2e.assert_cli_balances('maker', lambda: self.BALANCES)
        clock = iter(range(0, 1000, 20))
        with patch.object(e2e, 'lez_account_balance', lambda account: 1), \
                patch.object(e2e.time, 'sleep', lambda seconds: None), \
                patch.object(e2e.time, 'time', lambda: next(clock)):
            with self.assertRaisesRegex(e2e.Failure, 'reads 990 LEZ units for Acc1, the indexer 1'):
                e2e.assert_cli_balances('maker', lambda: self.BALANCES)

    def test_the_wrong_owner_table_names_balances_for_both_roles(self):
        self.assertIn(('balances',), e2e.MAKER_CLI_OWNER_COMMANDS)
        self.assertIn(('balances',), e2e.TAKER_CLI_OWNER_COMMANDS)
        self.assertIn(('health',), e2e.MAKER_CLI_OWNER_COMMANDS)


class ManualRefundAndClaim(unittest.TestCase):
    def test_they_are_registered_and_the_claim_needs_the_hooks(self):
        self.assertIs(e2e.SCENARIOS['maker-cli-refund'], e2e.scenario_maker_cli_refund)
        self.assertIs(e2e.ZEC_SCENARIOS['zec-maker-cli-refund'], e2e.scenario_zec_maker_cli_refund)
        self.assertIs(e2e.ZEC_SCENARIOS['zec-maker-cli-claim'], e2e.scenario_zec_maker_cli_claim)
        self.assertIn('zec-maker-cli-claim', e2e.NATIVE_REFUSED)
        self.assertEqual(e2e.release_unavailable('node:zec:zec-maker-cli-claim:TakerSellsLez'), e2e.RELEASE_HOOKS)
        self.assertIsNone(e2e.release_unavailable('node:zec:zec-maker-cli-refund:TakerSellsForeign'))
        self.assertIsNone(e2e.release_unavailable('node:btc:maker-cli-refund:TakerSellsForeign'))

    def test_each_refuses_the_direction_it_has_nothing_to_show_in(self):
        nothing = lambda *a, **k: self.fail('nothing may run')
        with patch.object(e2e, 'zec_taker_sells_lez', lambda: True), patch.object(e2e, 'zec_provision', nothing), \
                patch.object(e2e, 'lez_balances', nothing):
            with self.assertRaisesRegex(e2e.Failure, 'selling-ZEC scenario .*refunds its Zcash by itself'):
                e2e.scenario_zec_maker_cli_refund('1700000000')
        with patch.object(e2e, 'zec_taker_sells_lez', lambda: False), \
                patch.object(e2e, 'require_sidecar_test_hooks', nothing):
            with self.assertRaisesRegex(e2e.Failure, 'selling-LEZ'):
                e2e.scenario_zec_maker_cli_claim('1700000000')
        self.assertFalse(e2e.MAKER_CLI)

    def test_the_refusals_the_claim_scenario_expects_are_the_stores(self):
        store = (REPO / 'crates/swap-store/src/maker_actor_process.rs').read_text()
        self.assertIn(f'#[error("maker actor {e2e.MANUAL_ACTION_PENDING}")]', store)
        self.assertIn(f'#[error("maker actor manual action {e2e.STALE_GENERATION}")]', store)
        node = (REPO / 'crates/maker-node/src/lib.rs').read_text()
        self.assertIn('const CONFLICT: i32 = -32_009;', node)
        # pending and stale are conflicts; a terminal actor is an invalid request (-32602)
        self.assertRegex(node, r'ManualActionGenerationConflict\s*\| MakerActorProcessError::ManualActionPending => '
                               r'rpc_error\(CONFLICT')
        self.assertRegex(node, r'MakerActorProcessError::ManualActionUnavailable\s*\|[^=]*=> invalid_request\(error\)')
        self.assertIn('rpc_error(-32_602, error.to_string())', node)

    def test_the_node_runs_the_same_claim_command_by_itself(self):
        """What zec-maker-cli-claim's docstring says it does not show."""
        runtime = (REPO / 'crates/maker-node/src/actor_supervisor/runtime.rs').read_text()
        self.assertRegex(runtime, r'matches!\(next_action, "claim_lez" \| "claim_zcash"\)\s*\{\s*'
                                  r'StatusDecision::Run\(ActorEffectCommand::Claim\)')
        self.assertRegex(runtime, r'\(MakerActorKindV1::Monero \| MakerActorKindV1::Zcash, MakerActorManualAction::Claim\) => '
                                  r'\{\s*ActorEffectCommand::Claim')
        # an action is never spliced into the attempt that is running
        store = (REPO / 'crates/swap-store/src/maker_actor_process.rs').read_text()
        self.assertIn('if lease.generation == action.requested_after_generation {', store)

    def test_the_operators_evidence_needs_the_request_completed_and_history_at_the_phase(self):
        views = {'monitor': {'schedule_state': 'terminal', 'progress': {'observation': {'phase': 'maker_leg_refunded'}},
                             'manual_action': {'request_id': 'e2e-maker-refund-1', 'action': 'refund', 'state': 'completed'}},
                 'history': [{'id': SWAP, 'phase': 'MakerLegRefunded'}, {'id': 'other', 'phase': 'Completed'}]}
        with patch.object(e2e, 'maker_cli', lambda command, *_: views[command]):
            read = e2e.maker_cli_action_evidence(SWAP, 'e2e-maker-refund-1', 'refund', e2e.MAKER_REFUNDED_PHASES)
            self.assertEqual(read['phase'], 'maker_leg_refunded')
            self.assertEqual(read['history'], {'id': SWAP, 'phase': 'MakerLegRefunded'})
            with self.assertRaisesRegex(e2e.Failure, 'does not show the refund request other-request completed'):
                e2e.maker_cli_action_evidence(SWAP, 'other-request', 'refund', e2e.MAKER_REFUNDED_PHASES)
            with self.assertRaisesRegex(e2e.Failure, 'terminal at maker_leg_refunded, not'):
                e2e.maker_cli_action_evidence(SWAP, 'e2e-maker-refund-1', 'refund', {'completed'})
            views['monitor']['manual_action']['state'] = 'queued'
            with self.assertRaisesRegex(e2e.Failure, 'does not show the refund request'):
                e2e.maker_cli_action_evidence(SWAP, 'e2e-maker-refund-1', 'refund', e2e.MAKER_REFUNDED_PHASES)
            views['monitor']['manual_action']['state'] = 'completed'
            views['history'][0]['phase'] = 'Refunded'
            with self.assertRaisesRegex(e2e.Failure, 'history does not list'):
                e2e.maker_cli_action_evidence(SWAP, 'e2e-maker-refund-1', 'refund', e2e.MAKER_REFUNDED_PHASES)
        phases = (REPO / 'crates/swap-core/src/lib.rs').read_text()
        for phase in e2e.MAKER_REFUNDED_PHASES | {'completed'}:
            self.assertRegex(phases, rf'\n    {e2e.pascal(phase)},')


class OutagePublish(unittest.TestCase):
    """R8, the Maker's side: a publish under the outage is committed, fails
    with the Delivery adapter's message, and reaches the Taker once Delivery is
    back with no second publish."""
    ACTIVE = [{'offer': {'id': 'offer-e2e-outage-17'}, 'status': 'active', 'revision': 1}]
    REFUSED_LIST = {'error': {'code': -32010, 'message': 'Taker dependency unavailable',
                              'data': {'category': 'authenticated_delivery_unavailable'}}}

    def publish(self, reply, offers=None, listing=None):
        with patch.object(e2e, 'ROUTE', dict(BTC)), patch.object(e2e, 'maker_cli_reply', lambda arguments: reply), \
                patch.object(e2e, 'maker_cli', lambda *argv: self.ACTIVE if offers is None else offers), \
                patch.object(e2e, 'rpc', lambda *a, **k: self.REFUSED_LIST if listing is None else listing):
            return e2e.publish_under_outage('17')

    def test_the_documented_failure_with_the_offer_committed_passes(self):
        published = self.publish({'error': {'code': -32603, 'message': e2e.DELIVERY_REFUSED_DIRECTORY}})
        self.assertEqual(published['request'], {'request_id': 'e2e-outage-publish-17', 'offer_id': 'offer-e2e-outage-17',
                                                'route': BTC})
        self.assertEqual((published['status'], published['revision']), ('active', 1))
        self.assertEqual(published['taker_offer_list'], {'code': -32010, 'category': 'authenticated_delivery_unavailable'})

    def test_anything_else_fails(self):
        refused = {'error': {'code': -32603, 'message': e2e.DELIVERY_REFUSED_DIRECTORY}}
        with self.assertRaisesRegex(e2e.Failure, 'did not fail with -32603'):
            self.publish({'result': {'revision': 1, 'was_replay': False}})
        with self.assertRaisesRegex(e2e.Failure, 'did not fail with -32603'):
            self.publish({'error': {'code': -32602, 'message': 'chain dependency is unavailable for route'}})
        with self.assertRaisesRegex(e2e.Failure, 'does not list offer-e2e-outage-17 once, active'):
            self.publish(refused, offers=[])
        with self.assertRaisesRegex(e2e.Failure, 'was not refused as authenticated_delivery_unavailable'):
            self.publish(refused, listing={'result': {'schema_version': 1, 'offers': []}})

    def test_the_message_and_the_order_are_the_nodes(self):
        delivery = (REPO / 'crates/node-common/src/run_local_delivery.rs').read_text()
        self.assertIn(f'#[error("{e2e.DELIVERY_REFUSED_DIRECTORY}")]', delivery)
        node = (REPO / 'crates/maker-node/src/lib.rs').read_text()
        # the store commits, then Delivery is written; a Delivery failure is INTERNAL_ERROR (-32603)
        self.assertLess(node.index('let commit = publish_offer(&context, &request, now_unix_seconds)?;'),
                        node.index('publish_offer_to_delivery(&context, &offer_id)?;'))
        self.assertIn('rpc_error(INTERNAL_ERROR, error.to_string())', node)
        # every maker_health call writes the files the store has and the directory lacks
        health = node[node.index('"maker_health",'):node.index('fn route_dependency_state')]
        self.assertIn('reconcile_unhealthy_route_offers(&context, now_unix_seconds)?;', health)
        self.assertIn('let _ = delivery.reconcile(&projected, now_unix_seconds);', node)

    def test_after_the_restore_the_offer_is_discovered_replayed_and_withdrawn(self):
        ran = []

        def maker_cli(*argv):
            ran.append(argv)
            if argv[0] == 'offers':
                return self.ACTIVE
            return {'revision': 1, 'was_replay': argv[0] == 'publish-offer'}

        published = {'request': {'request_id': 'e2e-outage-publish-17', 'offer_id': 'offer-e2e-outage-17', 'route': BTC},
                     'status': 'active', 'revision': 1}
        with patch.object(e2e, 'discover', lambda offer_id, timeout: {'signed_envelope_sha256': [1, 2]}), \
                patch.object(e2e, 'maker_cli', maker_cli):
            recovered = e2e.assert_outage_offer_recovered(published)
        self.assertTrue(recovered['replay']['was_replay'])
        self.assertEqual(ran, [
            ('offers',),
            ('publish-offer', '--request-id', 'e2e-outage-publish-17', '--offer-id', 'offer-e2e-outage-17',
             '--pair', 'bitcoin', '--direction', 'taker-sells-foreign'),
            ('withdraw-offer', '--request-id', 'e2e-outage-withdraw-17', '--offer-id', 'offer-e2e-outage-17',
             '--expected-revision', '1')])
        with patch.object(e2e, 'discover', lambda offer_id, timeout: {}), \
                patch.object(e2e, 'maker_cli', lambda *argv: self.ACTIVE if argv[0] == 'offers' else
                             {'revision': 1, 'was_replay': False}):
            with self.assertRaisesRegex(e2e.Failure, 'is not a replay of the committed offer'):
                e2e.assert_outage_offer_recovered(published)


class StalePriceModule(unittest.TestCase):
    def test_it_is_registered_and_not_run_from_a_bundle(self):
        self.assertIs(e2e.SCENARIOS['maker-price-module-stale'], e2e.scenario_maker_price_module_stale)
        self.assertIn('TEST price module', e2e.release_unavailable('node:btc:maker-price-module-stale:TakerSellsForeign'))

    def test_the_stale_build_is_older_than_the_nodes_bound_and_fails_validation(self):
        fixture = (REPO / 'crates/logos-price-c-api/tests/fixtures/logos_price_fixture.c').read_text()
        self.assertRegex(fixture, r'#if defined\(FIXTURE_STALE\)\s*response->as_of_unix_seconds = '
                                  r'request->now_unix_seconds - 61u;')
        # the fixture echoes the route it is asked for: it prices a Zcash route too
        self.assertIn('response->pair = request->pair;', fixture)
        node = (REPO / 'crates/maker-node/src/bin/lez-maker-node.rs').read_text()
        self.assertIn('arguments.logos_price_max_age_seconds.unwrap_or(30)', node)
        # the launcher passes no bound of its own
        self.assertNotIn('logos-price-max-age', (REPO / 'modules/lez-role-node/node-entrypoint.sh').read_text())
        source = (REPO / 'crates/maker-node/src/price_source.rs').read_text()
        self.assertIn(f'#[error("{e2e.PRICE_SOURCE_INVALID}")]', source)
        lib = (REPO / 'crates/maker-node/src/lib.rs').read_text()
        self.assertRegex(lib, r'PriceSourceError::DuplicateQuote \| PriceSourceError::InvalidSource => \{\s*'
                              r'rpc_error\(INTERNAL_ERROR')

    def test_a_route_with_the_module_needs_no_local_price(self):
        store = (REPO / 'crates/swap-store/src/maker_application.rs').read_text()
        self.assertRegex(store, r'if configuration\.enabled\(\)\s*&& configuration\.price_source\(\) == '
                                r'MakerPriceSourceKind::Local\s*&& !route_exists')

    def test_a_pair_configuration_goes_back_as_pairs_printed_it(self):
        zcash = e2e.PRICE_MODULE_ZEC_ROUTE
        listed = [{'revision': 9, 'value': {'route': zcash}}]
        value = {'route': zcash, 'enabled': True, 'price_source': 'logos_c_api', 'minimum_foreign_units': 100_000_000,
                 'maximum_foreign_units': 100_000_000, 'offer_ttl_seconds': 3600}
        with patch.object(e2e, 'maker_cli', lambda *argv: listed):
            self.assertEqual(e2e.configure_pair_arguments('r-1', value), [
                'configure-pair', '--request-id', 'r-1', '--expected-revision', '9', '--pair', 'zcash', '--direction',
                'taker-sells-lez', '--enabled', 'true', '--price-source', 'logos-c-api', '--minimum-foreign-units',
                '100000000', '--maximum-foreign-units', '100000000', '--offer-ttl-seconds', '3600'])
        with patch.object(e2e, 'maker_cli', lambda *argv: []):
            created = e2e.configure_pair_arguments('r-2', {**value, 'enabled': False, 'price_source': 'local'})
        self.assertNotIn('--expected-revision', created)
        self.assertEqual(created[created.index('--enabled') + 1], 'false')
        self.assertEqual(created[created.index('--price-source') + 1], 'local')


class ZecRefundKillPoints(unittest.TestCase):
    def test_both_roles_walk_the_refund_path_as_tests_of_their_own(self):
        seen = []
        with patch.object(e2e, 'scenario_zec_maker_refund', lambda stamp, kill_role=None: seen.append(kill_role) or {}):
            e2e.ZEC_SCENARIOS['zec-kill-maker-refund']('1')
            e2e.ZEC_SCENARIOS['zec-kill-taker-refund']('1')
        self.assertEqual(seen, ['maker', 'taker'])
        self.assertEqual(e2e.ZEC_REFUND_KILL_POINTS, ('both legs locked, refund times running', 'refund admitted'))
        # the take-time walk keeps its own points
        self.assertNotIn('refund admitted', [point for point, _ in e2e.ZEC_KILL_POINTS])

    def test_selling_zec_takes_the_role_along(self):
        seen = []
        with patch.object(e2e, 'zec_taker_sells_lez', lambda: False), \
                patch.object(e2e, 'scenario_zec_maker_refund_foreign', lambda stamp, kill_role=None: seen.append(kill_role) or {}):
            e2e.scenario_zec_maker_refund('1', kill_role='taker')
            e2e.scenario_zec_maker_refund('1')
        self.assertEqual(seen, ['taker', None])

    def refund(self, replies, states, **kill):
        views = iter(states)
        answers = iter(replies)
        killed_at = []

        def kill_at(role, swap_id, point, allowed):
            killed_at.append((role, point, allowed))
            return {'point': point}

        with patch.object(e2e, 'taker_view', lambda swap_id: {'state': next(views), 'progress_generation': 3}), \
                patch.object(e2e, 'rpc', lambda *a, **k: next(answers)), patch.object(e2e, 'kill_at', kill_at), \
                patch.object(e2e, 'maker_phase', lambda swap_id: 'both_legs_locked'), \
                patch.object(e2e, 'lez_chain_reading', lambda: {}), patch.object(e2e.time, 'sleep', lambda seconds: None), \
                patch.object(e2e, 'ZEC_REFUND_REQUESTS', {}), patch.object(e2e, 'log', lambda message: None):
            view = e2e.zec_refund_until_terminal(SWAP, '17', timeout=600, **kill)
        return view, killed_at

    def test_the_role_is_killed_once_when_the_refund_is_first_admitted(self):
        not_yet = {'error': {'code': -32010, 'data': {'category': 'taker_action_execution_unavailable'}}}
        killed = []
        view, killed_at = self.refund(
            [not_yet, {'result': {'was_replay': False}}, {'result': {'was_replay': True}}],
            ['refund_available', 'refund_available', 'refund_in_progress', 'refunded'], kill_role='maker', killed=killed)
        self.assertEqual(view['state'], 'refunded')
        self.assertEqual(killed_at, [('maker', 'refund admitted', e2e.ZEC_REFUND_ADMITTED_STATES)])
        self.assertEqual(killed, [{'point': 'refund admitted'}])

    def test_without_a_role_nothing_is_killed(self):
        _, killed_at = self.refund([{'result': {'was_replay': False}}], ['refund_available', 'refunded'])
        self.assertEqual(killed_at, [])

    def test_the_walk_must_have_killed_at_both_points_and_moved_no_lez(self):
        self.assertEqual(e2e.zec_refund_kills_settled(None, [], None, 'refunded', 'refunded'), {})
        with self.assertRaisesRegex(e2e.Failure, 'not at both refund points'):
            e2e.zec_refund_kills_settled('maker', [{'point': 'refund admitted'}], {}, 'refunded', 'refunded')
        kills = [{'point': point} for point in e2e.ZEC_REFUND_KILL_POINTS]
        settled = []
        with patch.object(e2e, 'assert_lez_settled',
                          lambda before, taker, maker: settled.append((before, taker, maker)) or {'outcome': taker}):
            summary = e2e.zec_refund_kills_settled('taker', kills, {'maker': 1, 'taker': 2}, 'refunded', 'maker_leg_refunded')
        self.assertEqual(settled, [({'maker': 1, 'taker': 2}, 'refunded', 'maker_leg_refunded')])
        self.assertEqual((summary['role'], summary['kill'], summary['outcome']), ('taker', kills, 'refunded'))


class Tiers(unittest.TestCase):
    """Where regress.sh runs the scenarios above, and on which profile."""
    NEW = ('node:btc:maker-cli-refund:TakerSellsForeign', 'node:zec:zec-maker-cli-refund:TakerSellsForeign',
           'node:zec:zec-maker-cli-claim:TakerSellsLez', 'node:btc:taker-cli:TakerSellsLez',
           'node:zec:zec-taker-cli-refund:TakerSellsLez', 'node:xmr:xmr-taker-cli:TakerSellsLez',
           'node:btc:outage-before-take:TakerSellsLez', 'node:zec:zec-outage-before-take:TakerSellsForeign',
           'node:btc:maker-price-module-stale:TakerSellsForeign',
           'node:zec:zec-kill-maker-refund:TakerSellsLez', 'node:zec:zec-kill-maker-refund:TakerSellsForeign',
           'node:zec:zec-kill-taker-refund:TakerSellsLez', 'node:zec:zec-kill-taker-refund:TakerSellsForeign')

    def tier(self, name):
        return subprocess.run(['bash', str(SCRIPTS / 'regress.sh'), 'list', name], capture_output=True, text=True,
                              check=True).stdout.split()

    def test_full_runs_each_once_and_the_smaller_tiers_none(self):
        full = self.tier('full')
        for test in self.NEW:
            self.assertEqual(full.count(test), 1, test)
        for tier in ('smoke', 'refunds'):
            self.assertFalse(set(self.NEW) & set(self.tier(tier)), tier)
        # each scenario only in the directions it has something to show in
        for absent in ('node:zec:zec-maker-cli-refund:TakerSellsLez', 'node:zec:zec-maker-cli-claim:TakerSellsForeign',
                       'node:btc:maker-price-module-stale:TakerSellsLez', 'node:xmr:xmr-taker-cli:TakerSellsForeign'):
            self.assertNotIn(absent, full)
        # the ones that were there keep their direction
        for kept in ('node:btc:taker-cli:TakerSellsForeign', 'node:btc:outage-before-take:TakerSellsForeign',
                     'node:zec:zec-outage-before-take:TakerSellsLez', 'node:zec:zec-taker-cli:TakerSellsLez'):
            self.assertIn(kept, full)

    def test_every_listed_scenario_is_one_the_harness_has(self):
        tables = {'btc': e2e.SCENARIOS, 'zec': e2e.ZEC_SCENARIOS, 'xmr': e2e.XMR_SCENARIOS}
        for test in self.NEW:
            _, pair, scenario, _ = test.split(':')
            self.assertIn(scenario, tables[pair], test)

    def test_each_runs_on_the_profile_its_journey_needs(self):
        script = (SCRIPTS / 'regress.sh').read_text()
        functions = ''.join(script[script.index(f'\n{name}() {{'):script.index('\n}\n', script.index(f'\n{name}() {{')) + 3]
                            for name in ('profile_of', 'expected_seconds'))
        probe = functions + '\nfor t in "$@"; do echo "$t $(profile_of "$t") $(expected_seconds "$t")"; done\n'
        listed = subprocess.run(['bash', '-c', probe, 'probe', *self.NEW], capture_output=True, text=True, check=True,
                                env={'PATH': os.environ['PATH']})
        rows = {row.split()[0]: row.split()[1:] for row in listed.stdout.splitlines()}
        want = {'maker-cli-refund': 'refund', 'zec-maker-cli-refund': 'zec-v6', 'zec-taker-cli-refund': 'zec-v6',
                'zec-kill-maker-refund': 'zec-v6', 'zec-kill-taker-refund': 'zec-v6'}
        for test in self.NEW:
            profile, seconds = rows[test]
            self.assertEqual(profile, want.get(test.split(':')[2], 'fast'), test)
            self.assertGreaterEqual(int(seconds), 250, test)
        # a refund journey waits for its refund times; the claim walk makes two swaps
        for test in self.NEW:
            if 'refund' in test:
                self.assertGreaterEqual(int(rows[test][1]), 1000, test)
        self.assertGreaterEqual(int(rows['node:zec:zec-maker-cli-claim:TakerSellsLez'][1]), 700)


if __name__ == '__main__':
    unittest.main()
