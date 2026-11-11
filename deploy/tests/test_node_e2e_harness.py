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


if __name__ == '__main__':
    unittest.main()
