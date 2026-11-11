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


if __name__ == '__main__':
    unittest.main()
