"""node-e2e.py's own logic that needs no stack (#103 items 8 to 14): the wait
evidence, the off-chain cut's commands, the settlement arithmetic.

Docker, bitcoin-cli and the Nodes are never called: the module is loaded and
its pure helpers are driven with fakes.
"""
import datetime
import importlib.util
import inspect
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock
from unittest.mock import patch

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


def load():
    clean = {k: v for k, v in os.environ.items() if not k.startswith('LEZ_')}
    with patch.dict(os.environ, clean, clear=True):
        spec = importlib.util.spec_from_file_location('node_e2e_harness', SCRIPTS / 'node-e2e.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return module


e2e = load()
READING = {'finalized': 3196, 'tip': 3199, 'source': 'heartbeat'}


class Clock:
    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now


class WaitEvidence(unittest.TestCase):
    def test_a_wait_line_names_what_it_waits_for_its_state_and_the_chain(self):
        line = e2e.format_wait_line("the Maker's refund", 'maker both_legs_locked', 75.4, READING)
        self.assertEqual(line, "  [75 s] waiting for the Maker's refund: maker both_legs_locked | "
                               "LEZ finalized 3196, tip 3199 (3 unfinalized) [heartbeat]")

    def test_an_unreadable_chain_is_logged_as_unknown(self):
        self.assertEqual(e2e.format_chain({'finalized': None, 'tip': 12, 'source': 'rpc'}),
                         'LEZ finalized ?, tip 12 [rpc]')

    def test_a_timeout_says_what_was_awaited_and_what_was_seen_last(self):
        message = e2e.timeout_message('the Taker to refund', 1800, 'taker refund_in_progress', READING)
        self.assertIn('timed out after 1800 s waiting for the Taker to refund', message)
        self.assertIn('last seen: taker refund_in_progress', message)
        self.assertIn('LEZ finalized 3196, tip 3199', message)
        self.assertNotIn('short-circuit', message)

    def test_a_wait_logs_each_change_and_otherwise_once_a_minute(self):
        clock, lines = Clock(), []
        with patch.object(e2e, 'log', lines.append), patch.object(e2e, 'lez_chain_reading', lambda: READING):
            wait = e2e.Wait('x', 600, clock=clock)
            self.assertTrue(wait.note('a'))
            clock.now += 3
            self.assertFalse(wait.note('a'))
            clock.now += 3
            self.assertTrue(wait.note('b'))
            clock.now += 59
            self.assertFalse(wait.note('b'))
            clock.now += 1
            self.assertTrue(wait.note('b'), 'a state that stands still is still logged every 60 s')
        self.assertEqual(len(lines), 3)
        self.assertTrue(all('LEZ finalized 3196, tip 3199' in line for line in lines))

    def test_a_costly_state_is_read_only_when_a_line_is_written(self):
        clock, reads = Clock(), []
        with patch.object(e2e, 'log', lambda line: None), patch.object(e2e, 'lez_chain_reading', lambda: READING):
            wait = e2e.Wait('x', 600, clock=clock)
            for _ in range(5):
                wait.note('same', lambda: reads.append(1) or 'detail')
                clock.now += 3
        self.assertEqual(len(reads), 1)

    def test_the_failure_carries_the_last_state(self):
        clock = Clock()
        with patch.object(e2e, 'log', lambda line: None), patch.object(e2e, 'lez_chain_reading', lambda: READING):
            wait = e2e.Wait('the Maker to complete', 30, clock=clock)
            wait.note('both_legs_locked', 'maker both_legs_locked')
            clock.now += 31
            self.assertFalse(wait.running())
            failure = wait.failure()
        self.assertIsInstance(failure, e2e.Failure)
        self.assertIn('last seen: maker both_legs_locked', str(failure))
        self.assertIsInstance(wait.failure(e2e.NotExercised), e2e.NotExercised)

    def test_the_chain_reading_prefers_a_fresh_heartbeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            runtime = pathlib.Path(tmp)
            (runtime / 'heartbeat').mkdir()
            (runtime / 'runtime.env').write_text('LEZ_HEARTBEAT_SECONDS=20\n')
            status = {'at_unix': int(time.time()), 'tip': 50, 'finalized': 47, 'stalled': False}
            (runtime / 'heartbeat' / 'status.json').write_text(json.dumps(status))
            with patch.object(e2e, 'RUNTIME', runtime), patch.object(e2e, 'lez_rpc_number') as rpc:
                self.assertEqual(e2e.lez_chain_reading(), {'finalized': 47, 'tip': 50, 'source': 'heartbeat'})
                rpc.assert_not_called()
                status['at_unix'] -= 600
                (runtime / 'heartbeat' / 'status.json').write_text(json.dumps(status))
                rpc.side_effect = [44, 52]
                self.assertEqual(e2e.lez_chain_reading(), {'finalized': 44, 'tip': 52, 'source': 'rpc'})
            urls = [call.args for call in rpc.call_args_list]
            self.assertEqual(urls, [(e2e.STACK_INDEXER_URL, 'getLastFinalizedBlockId'),
                                    (e2e.STACK_SEQUENCER_URL, 'getLastBlockId')])

    def test_the_maker_is_watched_long_enough_for_its_profile(self):
        self.assertEqual(e2e.maker_attention_seconds({'LEZ_MAKER_ACTOR_REQUEUE_SECONDS': '2',
                                                      'LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS': '10'}), 90)
        self.assertEqual(e2e.maker_attention_seconds({'LEZ_MAKER_ACTOR_REQUEUE_SECONDS': '',
                                                      'LEZ_MAKER_ACTOR_FAILURE_BACKOFF_SECONDS': ''}), 175)


class EarlyLockChecks(unittest.TestCase):
    """#103 item 8: what the early-lock scenarios read from the chain."""
    def test_only_what_the_wallet_sent_counts_as_a_lock(self):
        since = {'transactions': [{'txid': 'aa' * 32, 'category': 'receive'},
                                  {'txid': 'bb' * 32, 'category': 'send'},
                                  {'txid': 'bb' * 32, 'category': 'send'}]}
        with patch.object(e2e, 'bitcoin', lambda *args: since):
            self.assertEqual(e2e.wallet_sends_since('lez-maker', 'cc' * 32), ['bb' * 32])

    def test_the_wallet_is_asked_for_the_mempool_too(self):
        calls = []
        with patch.object(e2e, 'bitcoin', lambda *args: calls.append(args) or {'transactions': []}):
            e2e.wallet_sends_since('lez-maker', 'cc' * 32)
        self.assertEqual(calls, [('-rpcwallet=lez-maker', 'listsinceblock', 'cc' * 32, '1', 'false', 'true')])

    def run_lookup(self, stdout):
        result = unittest.mock.Mock(stdout=stdout, stderr='', returncode=0)
        with patch.object(e2e.subprocess, 'run', return_value=result) as run:
            found = e2e.lez_transaction_block('maker', 'ab' * 32)
        command = run.call_args.args[0]
        self.assertEqual(command[:3], ['docker', 'exec', 'lez-maker-node'])
        self.assertEqual(command[-1], 'http://127.0.0.1:3040/', "the Maker's own sequencer forwarder")
        self.assertEqual(json.loads(command[-2])['params'], ['ab' * 32])
        return found

    def test_an_included_lock_is_read_with_its_block(self):
        self.assertEqual(self.run_lookup('{"jsonrpc":"2.0","id":1,"result":[{"tx":1},4211]}'), 4211)
        self.assertIsNone(self.run_lookup('{"jsonrpc":"2.0","id":1,"result":null}'))

    def test_a_route_that_does_not_answer_is_a_failure_not_an_absence(self):
        with self.assertRaises(e2e.Failure):
            self.run_lookup('')
        with self.assertRaises(e2e.Failure):
            e2e.lez_transaction_block('maker', 'not-a-hash')


class OffchainCut(unittest.TestCase):
    """#103 item 9: the cut denies the shared inodes, the restore puts back the
    modes the Nodes check for, and the Maker's open Chat connections are read
    from its /proc/net/unix."""
    def test_the_cut_denies_the_socket_and_the_directory_in_the_makers_container(self):
        self.assertEqual(e2e.offchain_cut_commands('lez-l2'), [
            ['docker', 'exec', 'lez-l2-maker-node', 'chmod', '0000', '/run/lez/maker/chat.sock'],
            ['docker', 'exec', 'lez-l2-maker-node', 'chmod', '0000', '/delivery']])

    def test_the_restore_puts_back_the_modes_the_nodes_require(self):
        self.assertEqual(e2e.offchain_restore_commands('lez'), [
            ['docker', 'exec', 'lez-maker-node', 'chmod', '0600', '/run/lez/maker/chat.sock'],
            ['docker', 'exec', 'lez-maker-node', 'chmod', '0700', '/delivery']])

    def test_nothing_is_renamed_or_loosened(self):
        for command in e2e.offchain_cut_commands('lez') + e2e.offchain_restore_commands('lez'):
            self.assertNotIn('mv', command)
            self.assertNotIn('0755', command)

    def test_only_accepted_connections_on_the_chat_socket_count(self):
        listing = (
            'Num       RefCount Protocol Flags    Type St Inode Path\n'
            '0000000000000000: 00000002 00000000 00010000 0001 01 41001 /run/lez/maker/chat.sock\n'
            '0000000000000000: 00000003 00000000 00000000 0001 03 41002 /run/lez/maker/chat.sock\n'
            '0000000000000000: 00000003 00000000 00000000 0001 03 41003 /run/lez/maker/node.sock\n'
            '0000000000000000: 00000003 00000000 00000000 0001 03 41004\n')
        self.assertEqual(e2e.connected_chat_sockets(listing), 1)
        self.assertEqual(e2e.connected_chat_sockets(listing.splitlines()[0]), 0)

    def test_the_taker_probe_runs_as_the_containers_user_and_reads_both_channels(self):
        result = unittest.mock.Mock(stdout='{"chat": "EACCES", "delivery": "EACCES", "delivery_fds": 0, "uid": 4713}',
                                    stderr='', returncode=0)
        with patch.object(e2e.subprocess, 'run', return_value=result) as run:
            probe = e2e.taker_offchain_probe()
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ['docker', 'exec', 'lez-taker-node', 'python3'])
        self.assertNotIn('-u', command)
        self.assertEqual(command[-2:], ['/run/lez/maker/chat.sock', '/delivery'])
        self.assertEqual(probe['uid'], 4713)

    @unittest.skipIf(os.geteuid() == 0, 'root ignores the modes the cut sets')
    @unittest.skipUnless(os.path.isdir('/proc/self/fd'), 'the probe counts descriptors in /proc, as in the Taker container')
    def test_the_probe_sees_the_mode_cut_as_a_refusal(self):
        import socket
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as tmp:
            chat, delivery = os.path.join(tmp, 'chat.sock'), os.path.join(tmp, 'delivery')
            os.mkdir(delivery, 0o700)
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(chat)
            listener.listen(4)
            os.chmod(chat, 0o600)
            self.addCleanup(listener.close)

            def probe():
                out = subprocess.run([sys.executable, '-c', e2e.TAKER_OFFCHAIN_PROBE, chat, delivery],
                                     capture_output=True, text=True, check=True).stdout
                return json.loads(out)
            before = probe()
            self.assertEqual((before['chat'], before['delivery']), ('connected', 'readable'))
            os.chmod(chat, 0o000)
            os.chmod(delivery, 0o000)
            try:
                after = probe()
            finally:
                os.chmod(delivery, 0o700)
            self.assertEqual((after['chat'], after['delivery'], after['delivery_fds']), ('EACCES', 'EACCES', 0))


def deltas(maker_btc, maker_lez, taker_btc, taker_lez):
    return {'maker': {'btc_sat': maker_btc, 'lez_units': maker_lez},
            'taker': {'btc_sat': taker_btc, 'lez_units': taker_lez}}


class Settlement(unittest.TestCase):
    """#103 items 10 and 14: both parties on both chains, against the outcome."""
    LOT = {'foreign_units': 1_000_000, 'lez_units': 1_000}

    def problems(self, d, outcome, reverse):
        return e2e.settlement_problems(d, outcome, reverse, **self.LOT)

    def test_a_refund_returns_the_lock_less_the_lockers_fees_only(self):
        # forward the Taker locks Bitcoin: a lock and its refund cost it 4,100 sat (regression-m2-2026-11-11)
        self.assertEqual(self.problems(deltas(0, 0, -4_100, 0), 'refunded', False), [])
        self.assertEqual(self.problems(deltas(-4_100, 0, 0, 0), 'refunded', True), [])

    def test_a_refund_that_did_not_come_back_is_a_loss(self):
        problems = self.problems(deltas(0, 0, -1_004_100, 0), 'refunded', False)
        self.assertEqual(len(problems), 1)
        self.assertIn('taker BTC -1004100 sat, expected +0 less at most 20000 sat of fees', problems[0])

    def test_the_party_that_sent_no_bitcoin_must_not_move_a_satoshi(self):
        self.assertTrue(self.problems(deltas(-1, 0, -4_100, 0), 'refunded', False))

    def test_lez_moves_to_the_unit(self):
        self.assertTrue(self.problems(deltas(0, -1, -4_100, 0), 'refunded', False))

    def test_a_completed_swap_trades_the_lots(self):
        # forward: the Taker pays Bitcoin and receives LEZ; the Maker claims the Bitcoin less its claim fee
        self.assertEqual(self.problems(deltas(1_000_000 - 1_500, -1_000, -1_000_000 - 2_000, 1_000),
                                       'completed', False), [])
        # reversed: the Taker pays LEZ and claims the Maker's Bitcoin
        self.assertEqual(self.problems(deltas(-1_000_000 - 2_000, 1_000, 1_000_000 - 1_500, -1_000),
                                       'completed', True), [])

    def test_no_party_gains_more_than_the_lot(self):
        self.assertTrue(self.problems(deltas(1_000_001, -1_000, -1_000_002, 1_000), 'completed', False))

    def test_a_party_holding_both_lots_fits_no_outcome(self):
        # reversed: the Maker refunded its Bitcoin and also took the Taker's LEZ
        doubly = deltas(-4_000, 1_000, 0, -1_000)
        self.assertIsNone(e2e.classify_settlement(doubly, True, **self.LOT))
        self.assertEqual(e2e.classify_settlement(deltas(-4_000, 0, 0, 0), True, **self.LOT), 'refunded')
        self.assertEqual(e2e.classify_settlement(deltas(-1_002_000, 1_000, 998_500, -1_000), True, **self.LOT),
                         'completed')

    def test_deltas_are_after_minus_before(self):
        before = {'maker': {'btc_sat': 10, 'lez_units': 5}, 'taker': {'btc_sat': 7, 'lez_units': 1}}
        after = {'maker': {'btc_sat': 4, 'lez_units': 5}, 'taker': {'btc_sat': 7, 'lez_units': 3}}
        self.assertEqual(e2e.settlement_deltas(before, after), deltas(-6, 0, 0, 2))

    def test_a_wallet_counts_pending_and_immature_coins_too(self):
        balances = {'mine': {'trusted': 1.5, 'untrusted_pending': 0.00000123, 'immature': 0.25}}
        with patch.object(e2e, 'bitcoin', lambda *args: balances):
            self.assertEqual(e2e.wallet_total_sat('lez-maker'), 175_000_123)


class RefundAdmission(unittest.TestCase):
    """#103 item 10: what the Taker had seen when its refund was admitted."""
    def test_a_lez_maker_lock_in_the_view_is_observed(self):
        view = {'state': 'awaiting_second_lock', 'effects': [{'kind': 'taker_lock', 'chain': 'Bitcoin'},
                                                             {'kind': 'maker_lock', 'chain': 'Lez',
                                                              'transaction_id': 'ab' * 32, 'confirmations': 0}]}
        with self.assertRaises(e2e.Failure):
            e2e.assert_maker_lock_unobserved(view, 'now', None)

    def test_an_unconfirmed_bitcoin_maker_lock_is_not_yet_a_lock_to_rely_on(self):
        view = {'state': 'awaiting_second_lock', 'effects': [{'kind': 'maker_lock', 'chain': 'Bitcoin',
                                                              'transaction_id': 'ab' * 32, 'confirmations': 0}]}
        e2e.assert_maker_lock_unobserved(view, 'now', 1)
        view['effects'][0]['confirmations'] = 1
        with self.assertRaises(e2e.Failure):
            e2e.assert_maker_lock_unobserved(view, 'now', 1)

    def test_a_state_after_the_second_lock_is_observed(self):
        for state in ('both_legs_locked', 'claim_available'):
            with self.assertRaises(e2e.Failure):
                e2e.assert_maker_lock_unobserved({'state': state}, 'now', None)
        e2e.assert_maker_lock_unobserved({'state': 'awaiting_second_lock'}, 'now', None)

    def test_a_refund_the_node_will_not_admit_blind_fails(self):
        views = iter([{'state': 'awaiting_second_lock', 'progress_generation': 4}] * 3)
        reply = {'error': {'code': -32010, 'data': {'category': 'lez_unavailable'}}}
        with patch.object(e2e, 'taker_view', lambda swap: next(views)), \
                patch.object(e2e, 'rpc', lambda *a, **k: reply), patch.object(e2e, 'timing_profile', dict):
            with self.assertRaises(e2e.Failure) as refused:
                e2e.admit_refund_unobserved('ab' * 32, '1', None)
        self.assertIn('did not admit the refund', str(refused.exception))


class LateClaimSettlement(unittest.TestCase):
    """#103 item 14 (b): whichever way the swap ends, the chains must agree with it."""
    BEFORE = {'maker': {'btc_sat': 10_000_000, 'lez_units': 5_000},
              'taker': {'btc_sat': 10_000_000, 'lez_units': 5_000}}

    def settle(self, after, taker_state, maker_phase, reverse=True):
        with patch.object(e2e, 'party_balances', lambda: after), patch.object(e2e, 'REVERSE', reverse), \
                patch.object(e2e, 'log', lambda line: None):
            return e2e.assert_settled_consistently(self.BEFORE, taker_state, maker_phase)

    def after(self, maker_btc, maker_lez, taker_btc, taker_lez):
        return {role: {asset: self.BEFORE[role][asset] + delta for asset, delta in values.items()}
                for role, values in deltas(maker_btc, maker_lez, taker_btc, taker_lez).items()}

    def test_a_refund_both_nodes_report_passes(self):
        self.assertEqual(self.settle(self.after(-4_000, 0, 0, 0), 'refunded', 'refunded')['outcome'], 'refunded')

    def test_a_completion_both_nodes_report_passes(self):
        result = self.settle(self.after(-1_002_000, 1_000, 998_000, -1_000), 'completed', 'completed')
        self.assertEqual(result['outcome'], 'completed')

    def test_a_maker_with_both_lots_fails(self):
        with self.assertRaises(e2e.Failure) as failed:
            self.settle(self.after(-4_000, 1_000, 0, -1_000), 'refunded', 'refunded')
        self.assertIn('funds lost or taken twice', str(failed.exception))

    def test_balances_that_disagree_with_the_nodes_fail(self):
        with self.assertRaises(e2e.Failure):
            self.settle(self.after(-4_000, 0, 0, 0), 'completed', 'completed')
        with self.assertRaises(e2e.Failure):
            self.settle(self.after(-4_000, 0, 0, 0), 'refunded', 'completed')



class NativeNodes(unittest.TestCase):
    """LEZ_E2E_NATIVE_RUN_ROOT: the Nodes run on this host, not in containers."""

    def test_the_owner_socket_is_reached_with_this_hosts_curl(self):
        ran = []

        def fake_run(command, **_):
            ran.append(command)
            return unittest.mock.Mock(returncode=0, stdout='{"jsonrpc":"2.0","id":1,"result":{"ready":true}}', stderr='')

        with patch.object(e2e, 'NATIVE_RUN_ROOT', '/home/lez/run'), patch.object(e2e.subprocess, 'run', fake_run):
            self.assertEqual(e2e.call('taker', 'taker_health', {'schema_version': 1}), {'ready': True})
            e2e.wait_healthy('maker', timeout=5)
        self.assertEqual(ran[0][:6], ['curl', '-sS', '--max-time', '300', '--unix-socket', '/home/lez/run/taker/node.sock'])
        self.assertIn('"method": "maker_health"', ran[1][ran[1].index('--data') + 1])
        self.assertNotIn('docker', [part for command in ran for part in command])

    def test_without_it_the_container_is_exec_d(self):
        ran = []

        def fake_run(command, **_):
            ran.append(command)
            return unittest.mock.Mock(returncode=0, stdout='{"jsonrpc":"2.0","id":1,"result":1}', stderr='')

        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run):
            e2e.call('maker', 'maker_health', {})
        self.assertEqual(ran[0][:7], ['docker', 'exec', 'lez-maker-node', 'curl', '-sS', '--max-time', '300'])
        self.assertIn('/run/lez/maker/node.sock', ran[0])

    def test_a_scenario_that_needs_the_containers_is_refused(self):
        with tempfile.TemporaryDirectory() as runtime, \
                patch.object(e2e, 'NATIVE_RUN_ROOT', '/home/lez/run'), \
                patch.object(e2e, 'LANE_RUNTIME', pathlib.Path(runtime)), \
                patch.object(e2e, 'timing_profile', lambda: {}), \
                patch.object(e2e, 'wait_healthy', lambda *a, **k: self.fail('no Node may be asked')):
            self.assertFalse(e2e.run('offline-after-lock'))
            summary = json.loads((pathlib.Path(runtime) / 'e2e' / 'offline-after-lock.json').read_text())
        self.assertEqual(summary['result'], 'failed')
        self.assertIn('LEZ_E2E_NATIVE_RUN_ROOT', summary['error'])

    def test_a_command_in_the_node_runs_beside_the_native_node(self):
        files = {'maker': {'hold': '/home/lez/maker.hold', 'log': '/home/lez/maker.node.log'},
                 'taker': {'hold': '/home/lez/taker.hold', 'log': '/home/lez/taker.node.log'}}
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''):
            self.assertEqual(e2e.node_command('taker', ['cat', '/var/lib/lez/zec/corridor/x']),
                             ['docker', 'exec', 'lez-taker-node', 'cat', '/var/lib/lez/zec/corridor/x'])
        with patch.object(e2e, 'NATIVE_RUN_ROOT', '/home/lez/run'), \
                patch.object(e2e, 'NATIVE_HOME', pathlib.Path('/home/lez')), \
                patch.object(e2e, 'NATIVE_FILES', files), \
                patch.object(e2e, 'native_bin', lambda role: f'/pkg/{role}/bin'):
            self.assertEqual(e2e.node_command('taker', ['/usr/local/bin/lez-zec-taker-actor', '--config',
                                                        '/var/lib/lez/zec/corridor/taker/actor-config.json', 'drive']),
                             ['/pkg/taker/bin/lez-zec-taker-actor', '--config',
                              '/home/lez/zec/corridor/taker/actor-config.json', 'drive'])
            self.assertEqual(e2e.node_command('maker', ['cat', '/run/lez/maker/node.log']),
                             ['cat', '/home/lez/maker.node.log'])
            self.assertEqual(e2e.node_command('taker', ['python3', '-c', "glob('/var/lib/lez/taker/btc/swaps/*')"]),
                             ['python3', '-c', "glob('/home/lez/taker/btc/swaps/*')"])

    def test_a_native_node_is_stopped_by_its_hold_file_and_started_by_removing_it(self):
        with tempfile.TemporaryDirectory() as home:
            hold = pathlib.Path(home) / 'taker.hold'
            files = {'maker': {'hold': str(pathlib.Path(home) / 'maker.hold'), 'log': ''},
                     'taker': {'hold': str(hold), 'log': ''}}
            pids = iter([4242, None])
            killed = []
            with patch.object(e2e, 'NATIVE_RUN_ROOT', home + '/run'), patch.object(e2e, 'NATIVE_FILES', files), \
                    patch.object(e2e, 'native_pid', lambda role: next(pids, None)), \
                    patch.object(e2e.os, 'kill', lambda pid, sig: killed.append((pid, sig))):
                e2e.node_stop('taker')
                self.assertTrue(hold.exists())
                self.assertEqual(killed, [(4242, 15)])
                e2e.node_start('taker')
                self.assertFalse(hold.exists())

    def test_a_native_log_is_read_from_where_it_stood_at_since(self):
        with tempfile.TemporaryDirectory() as home:
            log_file = pathlib.Path(home) / 'taker.node.log'
            log_file.write_text('before\n')
            files = {'maker': {'hold': '', 'log': ''}, 'taker': {'hold': '', 'log': str(log_file)}}
            clock = e2e.LogClock()
            clock.samples['taker'] = [(1000.0, 0), (2000.0, len('before\n'))]
            log_file.write_text('before\nafter\n')
            with patch.object(e2e, 'NATIVE_RUN_ROOT', home + '/run'), patch.object(e2e, 'NATIVE_FILES', files), \
                    patch.object(e2e, 'LOG_CLOCK', clock):
                since = datetime.datetime.fromtimestamp(2500, datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ')
                self.assertEqual(e2e.node_log_since('taker', since), 'after\n')


class ChainsOverRpc(unittest.TestCase):
    """Issue 152: native Nodes on Linux, and chains reached only over RPC."""

    def test_a_native_node_s_bin_is_its_executable_s_directory_on_linux(self):
        # Linux: `ps -o comm=` is the bare name; /proc/<pid>/exe is the program.
        with tempfile.TemporaryDirectory() as root:
            program = pathlib.Path(root) / 'modules/lez_taker_node/bin/lez-taker-node'
            program.parent.mkdir(parents=True)
            program.write_text('')
            exe = pathlib.Path(root) / 'exe'
            exe.symlink_to(program)
            real_path = pathlib.Path

            def proc(path):
                return exe if str(path) == '/proc/4242/exe' else real_path(path)

            with patch.object(e2e, 'NATIVE_RUN_ROOT', root + '/run'), \
                    patch.object(e2e, 'native_pid', lambda role: 4242), \
                    patch.object(e2e.pathlib, 'Path', proc), \
                    patch.object(e2e.subprocess, 'run', lambda *a, **k: self.fail('ps is not asked on Linux')):
                self.assertEqual(e2e.native_bin('taker'), str(program.parent))

    def test_bitcoin_cli_becomes_json_rpc_with_the_cookie(self):
        import http.server
        import threading
        seen = []

        class Core(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['content-length'])))
                seen.append((self.path, self.headers['authorization'], body))
                reply = ({'result': None, 'error': {'code': -5, 'message': 'No such mempool transaction'}}
                         if body['method'] == 'getrawtransaction' else {'result': {'ok': body['params']}, 'error': None})
                data = json.dumps({**reply, 'id': body['id']}).encode()
                self.send_response(500 if reply['error'] else 200)
                self.send_header('content-length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *_):
                pass

        server = http.server.HTTPServer(('127.0.0.1', 0), Core)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.shutdown)
        with tempfile.TemporaryDirectory() as root:
            cookie = pathlib.Path(root) / 'cookie'
            cookie.write_text('__cookie__:secret\n')
            with patch.object(e2e, 'CHAINS_RPC_ONLY', True), \
                    patch.object(e2e, 'BTC_RPC_URL', f'http://127.0.0.1:{server.server_port}/'), \
                    patch.object(e2e, 'BTC_RPC_COOKIE', cookie), \
                    patch.object(e2e.subprocess, 'run', lambda *a, **k: self.fail('no docker exec')):
                self.assertEqual(e2e.bitcoin('generatetoaddress', '2', 'bcrt1qminer'), {'ok': [2, 'bcrt1qminer']})
                self.assertEqual(e2e.bitcoin('-rpcwallet=lez-maker', '-named', 'send', 'outputs={"a":1}', 'fee_rate=2'),
                                 {'ok': {'outputs': {'a': 1}, 'fee_rate': 2}})
                with self.assertRaisesRegex(e2e.Failure, 'bitcoin-cli getrawtransaction failed: .*No such mempool'):
                    e2e.bitcoin('getrawtransaction', 'ab' * 32)
        self.assertEqual(seen[0][0], '/')
        self.assertEqual(seen[1][0], '/wallet/lez-maker')
        self.assertEqual(seen[0][1], 'Basic X19jb29raWVfXzpzZWNyZXQ=')

    def test_a_scenario_that_stops_a_chain_container_fails_saying_so(self):
        ran = []
        with patch.object(e2e, 'CHAINS_RPC_ONLY', True), \
                patch.object(e2e.subprocess, 'run', lambda command, **_: ran.append(command)):
            with self.assertRaisesRegex(e2e.Failure, 'the chains are not containers here'):
                e2e.docker('stop', e2e.MINER)
            # A Node's own container is still this host's.
            e2e.docker('restart', f'{e2e.PREFIX}-taker-node')
        self.assertEqual(ran, [['docker', 'restart', f'{e2e.PREFIX}-taker-node']])


class FailedTestNodeLogs(unittest.TestCase):
    """node-logs (regress.sh, a try that did not pass): bounded, Delivery dumps
    dropped and counted, anything shaped like a secret redacted."""

    def test_secret_shapes_are_redacted_and_counted(self):
        text = ('rpcpassword=c2VjcmV0LXZhbHVlLWZvci10ZXN0\n'
                '{"capability_token": "0123456789abcdef0123456789abcdef"}\n'
                'Authorization: Basic X19jb29raWVfXzpzZWNyZXQ=\n'
                '-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIBase64Body\n-----END EC PRIVATE KEY-----\n'
                'taker actor 0f1f91d21ec5: drive → projected maker_lock (phase both_legs_locked)\n'
                '{"event":"taker_actor_command_failed","step":"effect","class":"ObservationUnavailable"}\n')
        redacted, count = e2e.redact(text)
        self.assertEqual(count, 4)
        for secret in ('c2VjcmV0LXZhbHVlLWZvci10ZXN0', '0123456789abcdef0123456789abcdef',
                       'X19jb29raWVfXzpzZWNyZXQ=', 'MHcCAQEEIBase64Body'):
            self.assertNotIn(secret, redacted)
        self.assertIn('rpcpassword=[REDACTED]', redacted)
        # what explains a failure is kept as it was
        self.assertIn('drive → projected maker_lock (phase both_legs_locked)', redacted)
        self.assertIn('"class":"ObservationUnavailable"', redacted)

    def test_lines_are_bounded_after_the_delivery_dumps_are_dropped(self):
        dump = f'[delivery] {e2e.DELIVERY_DUMP} ' + 'x' * 23_000
        text = '\n'.join(f'line {n}' if n % 3 else dump for n in range(30))
        kept, dropped = e2e.bounded_lines(text, limit=5)
        self.assertEqual(dropped, 10)
        self.assertEqual(kept.splitlines(), ['line 23', 'line 25', 'line 26', 'line 28', 'line 29'])

    def test_both_nodes_saved_from_logs_and_health_only(self):
        ran = []

        def fake_run(argv, **_):
            ran.append(argv)
            return unittest.mock.Mock(stdout=f'{argv[1]} output token=abcdefabcdefabcdef12\n'.encode())

        with tempfile.TemporaryDirectory() as out, \
                patch.object(e2e, 'NATIVE_RUN_ROOT', ''), \
                patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'rpc', lambda role, *_, **__: {'result': {'ready': True, 'role': role}}):
            notes = e2e.keep_node_logs(pathlib.Path(out) / 'try.nodes')
            files = sorted(p.name for p in (pathlib.Path(out) / 'try.nodes').iterdir())
            container = (pathlib.Path(out) / 'try.nodes' / 'taker-container.log').read_text()
        self.assertEqual(files, [f'{role}-{kind}' for role in ('maker', 'taker')
                                 for kind in ('basecamp.log', 'container.log', 'health.json', 'node.log')])
        self.assertEqual(len(notes), 8)
        self.assertIn('token=[REDACTED]', container)
        self.assertEqual(ran[0], ['docker', 'logs', '-t', '--tail', '20000', f'{e2e.PREFIX}-maker-node'])
        # nothing but logs is read: no state directory, database, cookie or runtime/ file
        for argv in ran:
            self.assertFalse(any('/var/lib/lez' in part or 'runtime/' in part or 'cookie' in part for part in argv), argv)


class RecordingStartsWithoutOffers(unittest.TestCase):
    """withdraw-offers (ui-e2e.sh --record): every active offer is withdrawn
    through the desk's own owner RPC, each with its revision and its own
    request id."""

    def test_every_active_offer_is_withdrawn_at_its_revision(self):
        calls = []

        def fake_call(role, method, params, *_):
            calls.append((role, method, params))
            if method == 'maker_offer_list':
                return [{'offer': {'id': 'aa' * 32}, 'revision': 1}, {'offer': {'id': 'bb' * 32}, 'revision': 3}]
            return {}

        with patch.object(e2e, 'call', fake_call):
            withdrawn = e2e.withdraw_open_offers('20261121T000000Z')
        self.assertEqual(withdrawn, ['aa' * 32, 'bb' * 32])
        self.assertEqual(calls[0], ('maker', 'maker_offer_list', {'states': ['active']}))
        self.assertEqual(calls[1:], [
            ('maker', 'maker_offer_withdraw', {'request_id': 'ui-record-withdraw-20261121T000000Z-1',
                                               'offer_id': 'aa' * 32, 'expected_revision': 1}),
            ('maker', 'maker_offer_withdraw', {'request_id': 'ui-record-withdraw-20261121T000000Z-2',
                                               'offer_id': 'bb' * 32, 'expected_revision': 3}),
        ])

    def test_no_open_offer_withdraws_nothing(self):
        calls = []
        with patch.object(e2e, 'call', lambda role, method, params, *_: calls.append(method) or []):
            self.assertEqual(e2e.withdraw_open_offers('s'), [])
        self.assertEqual(calls, ['maker_offer_list'])


REPO = SCRIPTS.parents[1]
PROBE = 'lez-v02-escrow-claim-probe'


def probe_evidence(**changes):
    snapshot = {'sequencer_tip': 10, 'tip_timestamp_ms': 1_000, 'status': 'Funded', 'custody_balance': 50_000,
                'claimant_balance': 7, 'claimant_nonce': 1}
    evidence = {'schema': 'lez_v02_escrow_claim_probe_v1', 'test_only': True, 'stored_secret_digest': 'dd' * 32,
                'presented_preimage_matches_digest': False, 'submitted_before_refund_at': True,
                'transaction_id': 'ee' * 32, 'included': False, 'included_in_block': None, 'observed_seconds': 30,
                'submission': {'decision': 'accepted', 'error_code': None, 'error_message': None},
                'before': dict(snapshot), 'after': dict(snapshot), 'escrow_kept_custody': True}
    evidence.update(changes)
    return evidence


class MakerEarlyClaim(unittest.TestCase):
    """zec-maker-early-claim (TM-0030): what can be checked without a stack."""

    def test_it_is_a_zec_scenario_that_needs_the_containers(self):
        self.assertIs(e2e.ZEC_SCENARIOS['zec-maker-early-claim'], e2e.scenario_zec_maker_early_claim)
        self.assertIn('zec-maker-early-claim', e2e.NATIVE_REFUSED)

    def test_it_runs_under_the_v8_refund_profile(self):
        source = inspect.getsource(e2e.scenario_zec_maker_early_claim)
        self.assertIn('zec_provision(profile=zec_refund_profile())', source)
        self.assertIn('deterministic-local-devnet-v8', source)
        with patch.dict(e2e.ROUTE, {'direction': 'TakerSellsLez'}), patch.object(e2e, 'ZEC_PUBLIC', False):
            self.assertEqual(e2e.zec_refund_profile(), 'deterministic-local-devnet-v8')

    def test_it_reuses_the_neighbours_mechanisms(self):
        source = inspect.getsource(e2e.scenario_zec_maker_early_claim)
        for helper in ('zec_cut_maker_zebra_route()', 'zec_wait_until_phase(', 'wait_maker_phase(',
                       'wait_lez_final_through(', 'node_stop("maker")', 'zec_refund_until_terminal(',
                       'maker_action_with_fresh_generation("maker_actor_refund_v1"', 'wait_maker_terminal('):
            self.assertIn(helper, source)

    def test_selling_zec_it_refuses_before_it_provisions(self):
        with patch.dict(e2e.ROUTE, {'direction': 'TakerSellsForeign'}), \
                patch.object(e2e, 'zec_provision', lambda **_: self.fail('nothing may be provisioned')):
            with self.assertRaisesRegex(e2e.Failure, 'selling-LEZ scenario'):
                e2e.scenario_zec_maker_early_claim('1700000000')

    def test_the_escrow_swap_id_domain_is_the_sdks(self):
        sdk = (REPO / 'crates/zec-swap-sdk/src/lez_derivation.rs').read_text()
        self.assertIn('const LEZ_SWAP_ID_V1_DOMAIN: &[u8] = b"logos.gateway.lez-swap-id.v1\\0";', sdk)
        self.assertEqual(e2e.LEZ_SWAP_ID_V1_DOMAIN, b'logos.gateway.lez-swap-id.v1\0')

    def test_the_search_finds_every_form_and_skips_links(self):
        preimage, digest = bytes(range(32)), bytes(range(32, 64))
        with tempfile.TemporaryDirectory() as root:
            base = pathlib.Path(root)
            (base / 'actors').mkdir()
            (base / 'agreement.borsh').write_bytes(b'head' + digest + b'tail')
            (base / 'config.json').write_text(json.dumps({'secret_digest': digest.hex()}))
            # across a read boundary of the script (1 MiB)
            (base / 'actors' / 'store.sqlite3').write_bytes(b'\0' * ((1 << 20) - 10) + preimage.hex().upper().encode())
            (base / 'link').symlink_to(base / 'agreement.borsh')
            result = subprocess.run([sys.executable, '-c', e2e.ZEC_SECRET_SEARCH, root, str(base / 'absent')],
                                    input=f'{preimage.hex()}\n{digest.hex()}\n', capture_output=True, text=True,
                                    check=True)
            found = json.loads(result.stdout)
        names = {pathlib.Path(path).name: size for path, size in found['files']}
        self.assertEqual(names, {'agreement.borsh': 40, 'config.json': 85, 'store.sqlite3': (1 << 20) + 54})
        hits = {key: sorted(pathlib.Path(path).name for path in paths) for key, paths in found['hits'].items()}
        self.assertEqual(hits['preimage_hex_upper'], ['store.sqlite3'])
        self.assertEqual(hits['preimage_raw'] + hits['preimage_hex'] + hits['preimage_base64'], [])
        self.assertEqual(hits['digest_raw'], ['agreement.borsh'])
        self.assertEqual(hits['digest_hex'], ['config.json'])
        self.assertEqual(found['unreadable'], [[str(base / 'absent'), 'not a directory']])

    def test_the_secret_goes_to_the_search_on_stdin_only(self):
        seen = {}

        def fake_run(command, **kwargs):
            seen.update(command=command, input=kwargs.get('input'))
            return unittest.mock.Mock(returncode=0, stdout='{"files": []}', stderr='')

        preimage, digest = b'\x11' * 32, b'\x22' * 32
        with patch.object(e2e.subprocess, 'run', fake_run):
            e2e.zec_secret_search('maker', e2e.ZEC_MAKER_SEARCH_ROOTS, preimage, digest)
        self.assertEqual(seen['command'][:4], ['docker', 'exec', '-i', 'lez-maker-node'])
        self.assertNotIn(preimage.hex(), ' '.join(seen['command']))
        self.assertEqual(seen['input'], f'{preimage.hex()}\n{digest.hex()}\n')
        self.assertEqual(seen['command'][-3:], list(e2e.ZEC_MAKER_SEARCH_ROOTS))

    def probe(self, evidence, returncode=0):
        ran = []

        def fake_run(command, **_):
            ran.append(command)
            return unittest.mock.Mock(returncode=returncode, stdout=json.dumps(evidence) + '\n', stderr='')

        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'log', lambda *_: None):
            return e2e.zec_escrow_claim_probe('a zero preimage', 'aa' * 32, 'bb' * 32, '00' * 32), ran

    def test_the_claim_is_the_makers_through_its_own_sequencer_route(self):
        evidence, ran = self.probe(probe_evidence())
        self.assertTrue(evidence['escrow_kept_custody'])
        command = ran[0]
        self.assertEqual(command[:4], ['docker', 'exec', 'lez-maker-node', f'/usr/local/bin/{PROBE}'])
        self.assertEqual(command[command.index('--private-key-file') + 1],
                         '/var/lib/lez/zec/corridor/maker/lez-signer.key')
        self.assertEqual(command[command.index('--sequencer-url') + 1], 'http://127.0.0.1:3040/')

    def test_a_claim_the_escrow_paid_fails_the_scenario(self):
        paid = probe_evidence(escrow_kept_custody=False, included=True, included_in_block=11,
                              after={'status': 'Claimed', 'custody_balance': 0, 'claimant_balance': 50_007,
                                     'sequencer_tip': 11, 'tip_timestamp_ms': 2_000, 'claimant_nonce': 2})
        with self.assertRaisesRegex(e2e.Failure, 'TM-0030'):
            self.probe(paid, returncode=3)

    def test_a_claim_after_the_refund_time_proves_nothing(self):
        with self.assertRaisesRegex(e2e.Failure, 'refund time'):
            self.probe(probe_evidence(submitted_before_refund_at=False))

    def test_the_probe_is_a_registered_test_only_tool_outside_the_role_packages(self):
        catalog = json.loads((REPO / 'deploy/executables.json').read_text())['executables']
        entry = next(item for item in catalog if item['target'] == PROBE)
        self.assertEqual((entry['manifest'], entry['package'], entry['class'], entry['role'], entry['profiles']),
                         ('compat/lez-v0_2-sidecar/Cargo.toml', 'lez-v0-2-sidecar', 'reference', 'maker', []))
        self.assertTrue((REPO / f'compat/lez-v0_2-sidecar/src/bin/{PROBE}.rs').is_file())
        self.assertIn('TEST-ONLY', (REPO / f'compat/lez-v0_2-sidecar/src/bin/{PROBE}.rs').read_text())
        from_scratch = (SCRIPTS / 'from-scratch.sh').read_text()
        self.assertIn(f'--bin {PROBE}', from_scratch)
        self.assertIn(f'install -m 0755 "$PROVISION/sidecar/{PROBE}" "$DEPLOY_ROOT/images/maker-node/"', from_scratch)
        self.assertIn(f'images/maker-node/{PROBE}', (SCRIPTS / 'stage-assets.sh').read_text())
        dockerfile = (REPO / 'deploy/images/maker-node/Dockerfile').read_text()
        self.assertIn(f'COPY {PROBE} /payload/', dockerfile)
        self.assertIn('COPY --from=strip --chmod=0555 /payload/ /usr/local/bin/', dockerfile)
        self.assertNotIn(PROBE, (REPO / 'deploy/images/taker-node/Dockerfile').read_text())
        self.assertIn(PROBE, (REPO / '.github/workflows/release-images.yml').read_text())
        for packager in ('package-role-node.sh', 'package-role-node-darwin.sh'):
            self.assertNotIn(PROBE, (SCRIPTS / packager).read_text())


XMR_NEW_SCENARIOS = ('xmr-taker-clock-ahead', 'xmr-punish-replans', 'xmr-maker-claim-without-authorization',
                     'xmr-maker-early-punish', 'xmr-unfunded-punish', 'xmr-taker-cheats')


def xmr_probe_evidence(**changes):
    snapshot = {'sequencer_tip': 10, 'tip_timestamp_ms': 1_000, 'status': 'Funded', 'custody_balance': 50_000,
                'claimant_balance': 7, 'claimant_nonce': 1}
    evidence = {'schema': 'lez_v02_escrow_claim_probe_v1', 'test_only': True, 'effect': 'xmr-punish',
                'stored_secret_digest': None, 'presented_preimage_matches_digest': False,
                'submitted_before_refund_at': True, 'punish_at_ms': 5_000, 'submitted_before_punish_at': True,
                'transaction_id': 'ee' * 32, 'included': False, 'included_in_block': None, 'observed_seconds': 30,
                'submission': {'decision': 'accepted', 'error_code': None, 'error_message': None},
                'before': dict(snapshot), 'after': dict(snapshot), 'escrow_kept_custody': True}
    evidence.update(changes)
    return evidence


CREDIT_PROBE = 'lez-v02-custody-credit-probe'


def credit_evidence(**changes):
    before = {'sequencer_tip': 10, 'status': 'Funded', 'custody_balance': 1_000, 'sender_balance': 100_000}
    after = {'sequencer_tip': 12, 'status': 'Funded', 'custody_balance': 1_001, 'sender_balance': 99_999}
    evidence = {'schema': 'lez_v02_custody_credit_probe_v1', 'test_only': True, 'custody_account': 'cc' * 32,
                'escrow_amount': 1_000, 'credit_amount': 1, 'sender_is_party': False, 'credit_landed': True,
                'included': True, 'included_in_block': 12, 'transaction_id': 'ee' * 32,
                'submission': {'decision': 'accepted', 'error_code': None, 'error_message': None},
                'before': before, 'after': after}
    evidence.update(changes)
    return evidence


class XmrOwnRefundAndAdversaries(unittest.TestCase):
    """The Node's own refund, the devnet test hooks and the XMR adversarial
    scenarios: what can be checked without a stack."""

    def run_commands(self, returncode=0, stdout=''):
        ran = []

        def fake_run(command, **_):
            ran.append(command)
            return unittest.mock.Mock(returncode=returncode, stdout=stdout, stderr='')

        return ran, fake_run

    def test_the_scenarios_are_registered_and_need_the_containers(self):
        for name in XMR_NEW_SCENARIOS:
            self.assertIn(name, e2e.XMR_SCENARIOS)
            self.assertIn(name, e2e.NATIVE_REFUSED)
        for name in ('xmr-taker-refund', 'xmr-taker-refund-kill'):
            self.assertIn(name, e2e.NATIVE_REFUSED)

    def test_a_hook_is_written_whole_in_the_roles_state_and_cleared(self):
        ran, fake_run = self.run_commands()
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'log', lambda *_: None):
            e2e.xmr_hook('taker', 'clock-offset-ms', '60000')
            e2e.xmr_hook('maker', 'punish-nonce-thefts', None)
        self.assertEqual(ran[0][:5], ['docker', 'exec', 'lez-taker-node', 'sh', '-c'])
        self.assertEqual(ran[0][-4:], ['hook', '/var/lib/lez/taker/test-hooks', 'clock-offset-ms', '60000'])
        self.assertEqual(ran[1][-4:], ['hook', '/var/lib/lez/maker/test-hooks', 'punish-nonce-thefts', ''])
        with tempfile.TemporaryDirectory() as root:
            for value in ('60000', ''):
                subprocess.run(['sh', '-c', e2e.XMR_HOOK_SCRIPT, 'hook', f'{root}/hooks', 'clock-offset-ms', value],
                               check=True)
                path = pathlib.Path(root, 'hooks', 'clock-offset-ms')
                self.assertEqual(path.read_text() if path.exists() else None, '60000\n' if value else None)
            self.assertEqual(oct(pathlib.Path(root, 'hooks').stat().st_mode & 0o777), '0o700')

    def test_the_switch_is_written_where_the_entrypoint_reads_it_and_the_taker_restarted(self):
        ran, fake_run = self.run_commands()
        restarted = []
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'node_restart', restarted.append), patch.object(e2e, 'wait_healthy', lambda _: None), \
                patch.object(e2e, 'log', lambda *_: None):
            with e2e.xmr_owner_refund_only():
                self.assertEqual(ran[-1][-2:], ['/var/lib/lez/taker/xmr-automatic-refund', 'false'])
            self.assertEqual(ran[-1][-2:], ['/var/lib/lez/taker/xmr-automatic-refund', 'true'])
        self.assertEqual(restarted, ['taker', 'taker'])
        entrypoint = (REPO / 'modules/lez-role-node/node-entrypoint.sh').read_text()
        self.assertIn('"$state/xmr-automatic-refund"', entrypoint)
        self.assertIn('xmr_automatic_refund:$automatic', entrypoint)
        self.assertIn('export LEZ_TEST_HOOKS_DIR="$state/test-hooks"', entrypoint)
        self.assertEqual(e2e.XMR_NODE_STATE, {'maker': '/var/lib/lez/maker', 'taker': '/var/lib/lez/taker'})

    def test_the_owner_commanded_refunds_switch_the_nodes_own_refund_off(self):
        source = inspect.getsource(e2e.scenario_xmr_taker_refund)
        self.assertIn('with xmr_owner_refund_only():', source)
        never = inspect.getsource(e2e.scenario_xmr_maker_never_funds)
        self.assertNotIn('taker_swap_refund_v1', never)
        self.assertIn('own refund from R', never)

    def probe(self, evidence, returncode=0, expect_moved=False):
        ran, fake_run = self.run_commands(returncode, json.dumps(evidence) + '\n')
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'xmr_escrow_program_id', lambda: 'bb' * 32), \
                patch.object(e2e, 'log', lambda *_: None):
            return e2e.xmr_escrow_probe('xmr-punish', 'aa' * 32, expect_moved, 'before P'), ran

    def test_the_xmr_effect_is_the_makers_owner_through_its_own_sequencer_route(self):
        evidence, ran = self.probe(xmr_probe_evidence())
        self.assertTrue(evidence['escrow_kept_custody'])
        self.assertEqual(ran[0][:4], ['docker', 'exec', 'lez-maker-node', 'install'])
        command = ran[1]
        self.assertEqual(command[:4], ['docker', 'exec', 'lez-maker-node', f'/usr/local/bin/{PROBE}'])
        self.assertEqual(command[command.index('--effect') + 1], 'xmr-punish')
        self.assertEqual(command[command.index('--swap-id') + 1], 'aa' * 32)
        self.assertEqual(command[command.index('--private-key-file') + 1], e2e.XMR_PROBE_SIGNER)
        self.assertEqual(command[command.index('--sequencer-url') + 1], 'http://127.0.0.1:3040/')

    def test_an_escrow_that_moved_or_stayed_against_the_expectation_fails(self):
        moved = xmr_probe_evidence(escrow_kept_custody=False, included=True, included_in_block=11,
                                   after={'status': 'Claimed', 'custody_balance': 0, 'claimant_balance': 50_007,
                                          'sequencer_tip': 11, 'tip_timestamp_ms': 6_000, 'claimant_nonce': 2})
        with self.assertRaisesRegex(e2e.Failure, 'expected it to stay'):
            self.probe(moved, returncode=3)
        self.probe(moved, returncode=3, expect_moved=True)
        with self.assertRaisesRegex(e2e.Failure, 'expected it to move'):
            self.probe(xmr_probe_evidence(), expect_moved=True)
        with self.assertRaisesRegex(e2e.Failure, 'failed: exit 1'):
            self.probe(xmr_probe_evidence(), returncode=1)

    def test_the_probe_sends_the_xmr_effects_the_nodes_never_send(self):
        probe = (REPO / f'compat/lez-v0_2-sidecar/src/bin/{PROBE}.rs').read_text()
        for text in ('XmrPunish', 'XmrClaim', 'ZecEscrowInstruction::PunishNativeXmr',
                     'ZecEscrowInstruction::ClaimNativeXmr', 'TEST-ONLY'):
            self.assertIn(text, probe)

    def test_the_devnet_build_has_the_test_hooks_only_on_request(self):
        # The release side of this is test_release_workflow.NoTestHooksInARelease.
        from_scratch = (SCRIPTS / 'from-scratch.sh').read_text()
        self.assertIn('hooks=",lez-taker-node/test-hooks"', from_scratch)
        self.assertIn('hooks="--features test-hooks"', from_scratch)
        self.assertIn('test-hooks = ["pair-xmr"]', (REPO / 'crates/taker-node/Cargo.toml').read_text())
        self.assertIn('test-hooks = []', (REPO / 'compat/lez-v0_2-sidecar/Cargo.toml').read_text())

    def test_the_scenarios_with_hooks_fail_at_once_without_them(self):
        for scenario, role in ((e2e.scenario_xmr_taker_clock_ahead, 'taker'),
                               (e2e.scenario_xmr_punish_replans, 'maker'),
                               (e2e.scenario_xmr_taker_cheats, 'taker')):
            body = inspect.getsource(scenario).split('"""', 2)[2]
            self.assertEqual(body.lstrip().splitlines()[0], f'xmr_require_test_hooks("{role}")')
        ran = []
        with patch.object(e2e.subprocess, 'run', lambda argv, **_: ran.append(argv) or
                               subprocess.CompletedProcess(argv, 1, b'', b'')):
            with self.assertRaisesRegex(e2e.Failure, 'LEZ_TEST_HOOKS=1'):
                e2e.xmr_require_test_hooks('taker')
        self.assertEqual(ran[0][-3:], ['-qa', 'clock-offset-ms', '/usr/local/bin/lez-taker-node'])
        with patch.object(e2e.subprocess, 'run', lambda argv, **_: subprocess.CompletedProcess(argv, 0)):
            e2e.xmr_require_test_hooks('maker')
        for program, text in e2e.XMR_TEST_HOOK_BUILDS.values():
            # The text is read only by code the feature builds in.
            source = {'clock-offset-ms': 'crates/taker-node/src/taker_service/xmr_supervisor.rs',
                      'punish-nonce-thefts': 'compat/lez-v0_2-sidecar/src/bridge_runtime.rs'}[text]
            uses = [line for line in (REPO / source).read_text().splitlines()
                    if f'"{text}"' in line]
            self.assertEqual(len(uses), 1, (source, uses))

    def test_the_stolen_units_are_counted_against_the_lot(self):
        source = inspect.getsource(e2e.scenario_xmr_punish_replans)
        self.assertIn('XMR_PUNISH_NONCE_THEFTS', source)
        self.assertEqual(e2e.XMR_PUNISH_NONCE_THEFTS, 6)
        with tempfile.TemporaryDirectory() as root:
            slots = pathlib.Path(root, 'btc', 'swaps', 'x', 'sidecar', 'planner')
            slots.mkdir(parents=True)
            for index in (1, 6, 7):
                (slots / f'xmr-native-punish-supersession.{index}.v3.json').write_text('{}')
            (slots / 'xmr-native-punish-reservation.v3.json').write_text('{}')
            found = subprocess.run([sys.executable, '-c', e2e.XMR_SUPERSESSION_FINDER, root], capture_output=True,
                                   text=True, check=True)
        self.assertEqual(found.stdout.strip(), '3')

    def test_the_swap_record_reader_finds_the_swap_by_its_facts(self):
        with tempfile.TemporaryDirectory() as root:
            for name, swap in (('one', 'aa' * 32), ('two', 'bb' * 32)):
                directory = pathlib.Path(root, 'btc', 'swaps', name)
                directory.mkdir(parents=True)
                (directory / 'xmr-swap.json').write_text(json.dumps({'swap_id': swap, 'refund_at_ms': 7}))
            (pathlib.Path(root, 'btc', 'swaps', 'two') / 'xmr-taker-progress.json').write_text('{"refund_submitted": "16"}')
            read = lambda *argv: subprocess.run([sys.executable, '-c', e2e.XMR_SWAP_RECORD_READER, *argv, root],
                                                capture_output=True, text=True, check=False)
            self.assertEqual(json.loads(read('bb' * 32, 'xmr-taker-progress.json').stdout), {'refund_submitted': '16'})
            self.assertIsNone(json.loads(read('aa' * 32, 'xmr-taker-progress.json').stdout))
            self.assertEqual(json.loads(read('aa' * 32, 'xmr-swap.json').stdout)['refund_at_ms'], 7)
            self.assertEqual(read('cc' * 32, 'xmr-swap.json').returncode, 3)


class CustodyCredit(unittest.TestCase):
    """The *-custody-credit scenarios (TM-0032): what can be checked without a stack."""

    def tearDown(self):
        e2e.CUSTODY_CREDIT.clear()

    def record(self, depositor, units=1, evidence=None, error=None):
        done = unittest.mock.Mock()
        e2e.CUSTODY_CREDIT.update({'depositor': depositor, 'units': units, 'thread': done,
                                   'evidence': credit_evidence() if evidence is None else evidence, 'error': error})

    def test_six_scenarios_are_registered_with_their_pair(self):
        self.assertEqual(sorted(name for name in e2e.SCENARIOS if 'custody-credit' in name),
                         ['btc-custody-credit', 'btc-custody-credit-refund'])
        self.assertEqual(sorted(name for name in e2e.ZEC_SCENARIOS if 'custody-credit' in name),
                         ['zec-custody-credit', 'zec-custody-credit-refund'])
        self.assertEqual(sorted(name for name in e2e.XMR_SCENARIOS if 'custody-credit' in name),
                         ['xmr-custody-credit', 'xmr-custody-credit-refund'])

    def test_the_finalized_lez_time_is_the_indexers_block_header(self):
        # the sequencer's getBlock answers the encoded block (a string); the
        # indexer's getBlockById answers the header with its timestamp
        asked = []

        class Answer(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

        def urlopen(request, timeout):
            asked.append((request.full_url, json.loads(request.data)))
            return Answer(json.dumps({'jsonrpc': '2.0', 'id': 1, 'result': {
                'header': {'block_id': 500, 'timestamp': 1_795_277_043_886}, 'body': {}}}).encode())

        with patch.object(e2e, 'lez_rpc_number', lambda url, method: 500), \
                patch.object(e2e.urllib.request, 'urlopen', urlopen):
            self.assertEqual(e2e.lez_finalized_time_ms(), 1_795_277_043_886)
        self.assertEqual(asked, [(e2e.STACK_INDEXER_URL, {'jsonrpc': '2.0', 'id': 1, 'method': 'getBlockById',
                                                          'params': [500]})])

    def test_the_escrow_probe_signs_with_the_key_compose_mounts(self):
        # compose mounts the Maker's identity directory at /run-lez-identity/maker;
        # /run-lez-identity/lez-signer.key does not exist in the container
        compose = (SCRIPTS.parent / 'compose.yaml').read_text()
        self.assertIn('/lez/maker:/run-lez-identity/maker:ro', compose)
        self.assertEqual(e2e.XMR_MAKER_OWNER_KEY, '/run-lez-identity/maker/lez-signer.key')
        self.assertIn('XMR_MAKER_OWNER_KEY, XMR_PROBE_SIGNER', inspect.getsource(e2e.xmr_escrow_probe))

    def test_the_strangers_key_reaches_the_container_on_stdin(self):
        # docker exec without -i gives the command an empty stdin: the key
        # file was written empty and the probe refused it
        runs = []
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), \
                patch.object(e2e, 'custody_credit_key', lambda: b'71' * 32 + b'\n'), \
                patch.object(e2e, 'lez_owner_account', lambda role: 'owner'), \
                patch.object(e2e, 'escrow_program_id', lambda: 'aa' * 32), \
                patch.object(e2e.subprocess, 'run', lambda argv, **kw: runs.append((argv, kw)) or
                             subprocess.CompletedProcess(argv, 0, json.dumps(credit_evidence()) + '\n', '')):
            e2e.run_custody_credit_probe('taker', 1, 60)
        install = next((argv, kw) for argv, kw in runs if kw.get('input'))
        self.assertEqual(install[0][:4], ['docker', 'exec', '-i', 'lez-maker-node'])
        self.assertEqual(install[1]['input'], b'71' * 32 + b'\n')

    def test_the_evidence_of_a_landed_stranger_credit_is_accepted(self):
        evidence = e2e.parse_custody_credit_evidence(0, json.dumps(credit_evidence()) + '\n', 1)
        self.assertEqual(evidence['after']['custody_balance'], 1_001)

    def test_a_credit_that_did_not_land_or_was_not_a_strangers_fails(self):
        cases = [
            (3, credit_evidence(credit_landed=False), 'did not land'),
            (1, credit_evidence(), 'probe failed'),
            (0, credit_evidence(credit_amount=2), 'expected'),
            (0, credit_evidence(sender_is_party=True), 'not a stranger'),
            (0, credit_evidence(before={'sequencer_tip': 1, 'status': 'Funded', 'custody_balance': 999,
                                        'sender_balance': 5},
                                after={'sequencer_tip': 2, 'status': 'Funded', 'custody_balance': 1_000,
                                       'sender_balance': 4}), 'not funded'),
            (0, credit_evidence(schema='lez_v02_escrow_claim_probe_v1'), 'another schema'),
        ]
        for returncode, evidence, message in cases:
            with self.subTest(message), self.assertRaisesRegex(e2e.Failure, message):
                e2e.parse_custody_credit_evidence(returncode, json.dumps(evidence), 1)
        with self.assertRaisesRegex(e2e.Failure, 'no evidence'):
            e2e.parse_custody_credit_evidence(1, '', 1)

    def test_the_surplus_goes_to_the_claimant_on_a_claim_and_the_depositor_on_a_refund(self):
        self.assertEqual(e2e.custody_credit_surplus('completed'), {})
        self.record('taker')
        self.assertEqual(e2e.custody_credit_surplus('completed'), {'maker': 1})
        self.assertEqual(e2e.custody_credit_surplus('refunded'), {'taker': 1})
        self.record('maker', units=3)
        self.assertEqual(e2e.custody_credit_surplus('completed'), {'taker': 3})
        self.assertEqual(e2e.custody_credit_surplus('refunded'), {'maker': 3})

    def test_a_failed_credit_fails_the_settlement_check(self):
        self.record('taker', error=e2e.Failure('no funded native escrow'))
        with self.assertRaisesRegex(e2e.Failure, 'custody credit failed'):
            e2e.custody_credit_surplus('completed')

    def settle(self, after, taker_state='completed', surplus=None):
        before = {'maker': 10_000, 'taker': 10_000}
        with patch.object(e2e, 'REVERSE', True), patch.object(e2e, 'LEZ_UNITS', 1_000), \
                patch.object(e2e, 'lez_rpc_number', lambda *_: None), patch.object(e2e, 'log', lambda *_: None), \
                patch.object(e2e, 'lez_balances', lambda: after):
            return e2e.assert_lez_settled(before, taker_state, 'completed' if taker_state == 'completed' else 'refunded',
                                          surplus)

    def test_the_settlement_allows_exactly_the_credit_to_exactly_the_recipient(self):
        # selling LEZ, the Taker deposits: a claim pays the Maker the lot and the credit
        self.record('taker')
        self.assertEqual(self.settle({'maker': 11_001, 'taker': 9_000})['lez_deltas'], {'maker': 1_001, 'taker': -1_000})
        for after in ({'maker': 11_000, 'taker': 9_000}, {'maker': 11_002, 'taker': 9_000},
                      {'maker': 11_000, 'taker': 9_001}):
            with self.subTest(after), self.assertRaisesRegex(e2e.Failure, 'LEZ balances moved'):
                self.settle(after)
        # a refund returns the lot and the credit to the Taker
        self.assertEqual(self.settle({'maker': 10_000, 'taker': 10_001}, 'refunded')['lez_deltas'],
                         {'maker': 0, 'taker': 1})

    def test_without_a_credit_the_settlement_stays_exact(self):
        self.assertEqual(self.settle({'maker': 11_000, 'taker': 9_000})['lez_deltas'], {'maker': 1_000, 'taker': -1_000})
        with self.assertRaisesRegex(e2e.Failure, 'LEZ balances moved'):
            self.settle({'maker': 11_001, 'taker': 9_000})
        self.assertEqual(self.settle({'maker': 11_002, 'taker': 9_000}, surplus={'maker': 2})['lez_deltas'],
                         {'maker': 1_002, 'taker': -1_000})

    def test_the_key_reaches_the_container_on_stdin_and_is_removed(self):
        ran = []

        def fake_run(command, **kwargs):
            ran.append((command, kwargs.get('input')))
            if command[-1:] == ['-f'] or 'rm' in command:
                return unittest.mock.Mock(returncode=0, stdout='', stderr='')
            if CREDIT_PROBE in ' '.join(command):
                return unittest.mock.Mock(returncode=0, stdout=json.dumps(credit_evidence()) + '\n', stderr='')
            return unittest.mock.Mock(returncode=0, stdout=b'', stderr=b'')

        key = b'ab' * 32 + b'\n'
        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'log', lambda *_: None), patch.object(e2e, 'custody_credit_key', lambda: key), \
                patch.object(e2e, 'escrow_program_id', lambda: 'bb' * 32), \
                patch.object(e2e, 'lez_owner_account', lambda role: f'{role}-account'):
            e2e.run_custody_credit_probe('taker', 1, 600)
        install, probe, remove = ran
        self.assertEqual(install[0][:3], ['docker', 'exec', 'lez-maker-node'])
        self.assertEqual(install[1], key)
        command = probe[0]
        self.assertEqual(command[:4], ['docker', 'exec', 'lez-maker-node', f'/usr/local/bin/{CREDIT_PROBE}'])
        self.assertNotIn(key.decode().strip(), ' '.join(command))
        for flag, value in (('--depositor', 'taker-account'), ('--escrow-program-id', 'bb' * 32),
                            ('--private-key-file', e2e.CUSTODY_CREDIT_KEY), ('--amount', '1'),
                            ('--sequencer-url', 'http://127.0.0.1:3040/')):
            self.assertEqual(command[command.index(flag) + 1], value)
        self.assertEqual(remove[0][-3:], ['rm', '-f', e2e.CUSTODY_CREDIT_KEY])

    def test_the_key_is_removed_when_the_probe_fails(self):
        ran = []

        def fake_run(command, **kwargs):
            ran.append(command)
            if CREDIT_PROBE in ' '.join(command):
                raise subprocess.TimeoutExpired(command, 1)
            return unittest.mock.Mock(returncode=0, stdout=b'', stderr=b'')

        with patch.object(e2e, 'NATIVE_RUN_ROOT', ''), patch.object(e2e.subprocess, 'run', fake_run), \
                patch.object(e2e, 'log', lambda *_: None), patch.object(e2e, 'custody_credit_key', lambda: b'k'), \
                patch.object(e2e, 'escrow_program_id', lambda: 'bb' * 32), \
                patch.object(e2e, 'lez_owner_account', lambda role: role):
            with self.assertRaises(subprocess.TimeoutExpired):
                e2e.run_custody_credit_probe('maker', 1, 600)
        self.assertEqual(ran[-1][-3:], ['rm', '-f', e2e.CUSTODY_CREDIT_KEY])

    def test_the_depositor_is_the_taker_only_when_it_sells_lez(self):
        with patch.object(e2e, 'REVERSE', True):
            self.assertEqual(e2e.custody_credit_depositor(), 'taker')
        with patch.object(e2e, 'REVERSE', False):
            self.assertEqual(e2e.custody_credit_depositor(), 'maker')

    def test_a_refund_variant_selling_the_foreign_coin_refuses_before_it_starts(self):
        scenario = e2e.with_custody_credit(lambda stamp: self.fail('the scenario may not run'), refund=True)
        with patch.object(e2e, 'REVERSE', False), \
                patch.object(e2e, 'start_custody_credit', lambda *_: self.fail('no credit may start')):
            with self.assertRaisesRegex(e2e.Failure, 'TakerSellsLez'):
                scenario('1700000000')

    def test_the_escrow_program_falls_back_to_the_verifiers_pin(self):
        verifier = (REPO / 'scripts/verify-lez-v02-provisional.sh').read_text()
        with patch.dict(os.environ, {'LEZ_ESCROW_PROGRAM_ID': ''}):
            self.assertIn(f'expected_image_id="{e2e.escrow_program_id()}"', verifier)
        with patch.dict(os.environ, {'LEZ_ESCROW_PROGRAM_ID': 'cd' * 32}):
            self.assertEqual(e2e.escrow_program_id(), 'cd' * 32)

    def test_the_probe_ships_like_the_claim_probe_outside_the_role_packages(self):
        catalog = json.loads((REPO / 'deploy/executables.json').read_text())['executables']
        entry = next(item for item in catalog if item['target'] == CREDIT_PROBE)
        self.assertEqual((entry['manifest'], entry['package'], entry['class'], entry['role'], entry['profiles']),
                         ('compat/lez-v0_2-sidecar/Cargo.toml', 'lez-v0-2-sidecar', 'reference', 'maker', []))
        source = (REPO / f'compat/lez-v0_2-sidecar/src/bin/{CREDIT_PROBE}.rs').read_text()
        self.assertIn('TEST-ONLY', source)
        self.assertIn('lez_v02_custody_credit_probe_v1', source)
        from_scratch = (SCRIPTS / 'from-scratch.sh').read_text()
        self.assertIn(f'--bin {CREDIT_PROBE}', from_scratch)
        self.assertIn(f'install -m 0755 "$PROVISION/sidecar/{CREDIT_PROBE}" "$DEPLOY_ROOT/images/maker-node/"',
                      from_scratch)
        self.assertIn(f'images/maker-node/{CREDIT_PROBE}', (SCRIPTS / 'stage-assets.sh').read_text())
        self.assertIn(f'COPY {CREDIT_PROBE} /payload/', (REPO / 'deploy/images/maker-node/Dockerfile').read_text())
        self.assertNotIn(CREDIT_PROBE, (REPO / 'deploy/images/taker-node/Dockerfile').read_text())
        self.assertIn(CREDIT_PROBE, (REPO / '.github/workflows/release-images.yml').read_text())
        for packager in ('package-role-node.sh', 'package-role-node-darwin.sh'):
            self.assertNotIn(CREDIT_PROBE, (SCRIPTS / packager).read_text())


if __name__ == '__main__':
    unittest.main()
