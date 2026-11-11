"""node-e2e.py's own logic that needs no stack (#103 items 8 to 14): the wait
evidence, the off-chain cut's commands, the settlement arithmetic.

Docker, bitcoin-cli and the Nodes are never called: the module is loaded and
its pure helpers are driven with fakes.
"""
import importlib.util
import json
import os
import pathlib
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


if __name__ == '__main__':
    unittest.main()
