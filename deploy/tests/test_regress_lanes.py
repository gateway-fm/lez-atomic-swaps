"""regress.sh with LEZ_LANES: tests on several Maker/Taker lanes at once.

The scenarios, up.sh, lanes.sh and reset-swaps.sh are fakes that log what they
were asked; curl answers the finalized height from a counter that a fake
scenario can freeze. Nothing here needs a stack.
"""
import json
import os
import pathlib
import shutil
import stat
import subprocess
import tempfile
import time
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


def executable(path, text):
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


# A scenario (node-e2e.py and ui-e2e.sh alike): logs start and end with its lane
# and container prefix, then behaves as FAKE_PLAN says for its scenario name:
# {"sleep": s, "rc": n, "freeze_once": true}. freeze_once freezes the chain on
# its first run and then waits to be stopped.
FAKE_SCENARIO = '''#!/usr/bin/env python3
import json, os, pathlib, sys, time
args = [a for a in sys.argv[1:] if not a.startswith("--")]
words = sys.argv[1:]
direction = words[words.index("--direction") + 1]
scenario = next(a for a in args if a not in (direction, "Zcash"))
plan = json.loads(os.environ.get("FAKE_PLAN", "{}")).get(scenario, {})
state = pathlib.Path(os.environ["STATE"])
lane = os.environ.get("LEZ_LANE", "-")
def note(what):
    with open(os.environ["CALLS"], "a") as f:
        f.write(f"{what} {scenario}:{direction} lane={lane} prefix={os.environ.get('LEZ_CONTAINER_PREFIX', '-')} "
                f"runtime={os.environ.get('LEZ_LANE_RUNTIME', '-')} at={time.time():.2f}\\n")
note("start")
marker = state / f"froze-{scenario}-{direction}"
if plan.get("freeze_once") and not marker.exists():
    marker.touch(); (state / "frozen").touch()
    time.sleep(600)
time.sleep(plan.get("sleep", 0.5))
note("end")
sys.exit(plan.get("rc", 0))
'''
FAKE_CURL = '''#!/bin/sh
h="$STATE/height"; [ -s "$h" ] || echo 1 >"$h"
n="$(cat "$h")"
[ -e "$STATE/frozen" ] || { n=$((n + 1)); echo "$n" >"$h"; }
echo "{\\"jsonrpc\\":\\"2.0\\",\\"id\\":1,\\"result\\":$n}"
'''
FAKE_LOGGER = '''#!/bin/sh
echo "%s LEZ_LANE=${LEZ_LANE:-} LEZ_LANES=${LEZ_LANES:-} DEFER=${LEZ_BOOTSTRAP_DEFER_CLAIM:-} $*" >>"$CALLS"
%s
'''


class Runner(unittest.TestCase):
    """A deploy directory with regress.sh, lane.sh and the fakes."""
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bin = self.tmp / 'bin'
        self.bin.mkdir()
        self.deploy = self.tmp / 'deploy'
        scripts = self.deploy / 'scripts'
        scripts.mkdir(parents=True)
        for name in ('regress.sh', 'lane.sh'):
            shutil.copy(SCRIPTS / name, scripts)
        executable(scripts / 'node-e2e.py', FAKE_SCENARIO)
        executable(scripts / 'ui-e2e.sh', '#!/bin/sh\nexec python3 "$(dirname "$0")/node-e2e.py" "$@"\n')
        # up.sh: a new chain thaws the fake one; a profile switch rewrites runtime.env
        executable(scripts / 'up.sh', FAKE_LOGGER % ('up', 'rm -f "$STATE/frozen"\n'
                                                     '[ -z "$LEZ_TIMING_PROFILE" ] || sed -i "s/^LEZ_TIMING_PROFILE=.*/'
                                                     'LEZ_TIMING_PROFILE=$LEZ_TIMING_PROFILE/" "$(dirname "$0")/../runtime/runtime.env"'))
        # docker: Bedrock's storage price unreadable (no worn chain); nothing else is asked
        executable(self.bin / 'docker', '#!/bin/sh\nexit 1\n')
        executable(scripts / 'lanes.sh', FAKE_LOGGER % ('lanes', ''))
        executable(scripts / 'reset-swaps.sh', FAKE_LOGGER % ('reset', ''))
        executable(self.bin / 'curl', FAKE_CURL)
        runtime = self.deploy / 'runtime'
        runtime.mkdir()
        (runtime / 'runtime.env').write_text('LEZ_TIMING_PROFILE=fast\nMINE_INTERVAL=20\nLEZ_BTC_MAKER_LOCK_CUTOFF_SECONDS=240\n'
                                              'LEZ_BTC_EARLIER_REFUND_SECONDS=420\nLEZ_BTC_LATER_REFUND_SECONDS=840\n')
        self.state = self.tmp / 'state'
        self.state.mkdir()
        self.calls = self.tmp / 'calls.log'
        self.out = self.tmp / 'out'

    def regress(self, *tests, plan=None, **extra):
        env = {**os.environ, 'PATH': f'{self.bin}:{os.environ["PATH"]}', 'CALLS': str(self.calls),
               'STATE': str(self.state), 'FAKE_PLAN': json.dumps(plan or {}), 'LEZ_REGRESS_OUT': str(self.out),
               'LEZ_REGRESS_POLL_SECONDS': '1', 'LEZ_REGRESS_FLAT_SECONDS': '3',
               'LEZ_REGRESS_MOVING_WINDOW': '3', 'LEZ_REGRESS_KILL_GRACE': '1', **extra}
        for key in ('LEZ_TIMING_PROFILE', 'LEZ_LANE', 'LEZ_CONTAINER_PREFIX', 'LEZ_LANE_RUNTIME', 'LEZ_STACK_COMPOSE_ARGS', 'LEZ_VOLUME_PREFIX'):
            env.pop(key, None)
        started = time.time()
        run = subprocess.run(['bash', str(self.deploy / 'scripts' / 'regress.sh'), 'run', *tests], env=env,
                             capture_output=True, text=True, timeout=180)
        return run, time.time() - started

    def calls_of(self, kind):
        lines = self.calls.read_text().splitlines() if self.calls.exists() else []
        return [line for line in lines if line.startswith(kind + ' ')]

    def results(self):
        return [line.split('\t') for line in (self.out / 'results.tsv').read_text().splitlines()]


class Lanes(Runner):
    def test_one_lane_runs_one_test_at_a_time_as_before(self):
        run, _ = self.regress('node:btc:happy:TakerSellsLez', 'node:btc:replay:TakerSellsLez')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        starts, ends = self.calls_of('start'), self.calls_of('end')
        self.assertEqual(len(starts), 2)
        self.assertLess(float(ends[0].split('at=')[1]), float(starts[1].split('at=')[1]))
        self.assertEqual(self.calls_of('lanes'), [])
        self.assertTrue(all(len(row) == 5 for row in self.results()))

    def test_two_lanes_run_side_by_side_each_on_its_own_pair(self):
        plan = {s: {'sleep': 2} for s in ('happy', 'replay', 'survivor', 'concurrent')}
        tests = [f'node:btc:{s}:TakerSellsLez' for s in plan]
        run, elapsed = self.regress(*tests, plan=plan, LEZ_LANES='2')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertLess(elapsed, 4 * 2, 'four 2 s tests on two lanes take about 4 s, not 8')
        self.assertEqual(len(self.calls_of('lanes')), 1)
        self.assertIn('lanes LEZ_LANE= LEZ_LANES=2', self.calls_of('lanes')[0])
        self.assertTrue(self.calls_of('lanes')[0].endswith('up 2'))
        starts = self.calls_of('start')
        self.assertEqual(len(starts), 4)
        for line in starts:
            if 'lane=1 ' in line:
                self.assertIn('prefix=lez runtime=runtime ', line)
            else:
                self.assertIn('lane=2 prefix=lez-l2 runtime=runtime/lanes/2 ', line)
        self.assertEqual({row[5] for row in self.results()}, {'1', '2'})
        self.assertTrue(all(row[2] == 'PASSED' and len(row) == 7 for row in self.results()))
        # each lane resets its own pair before each test, never the other's
        for line in self.calls_of('reset'):
            self.assertIn('--abandon-open', line)
            self.assertRegex(line, r'LEZ_LANE=[12] ')

    def test_the_longest_tests_start_first(self):
        durations = self.tmp / 'durations.tsv'
        durations.write_text('node:btc:happy:TakerSellsLez\t1\tPASSED\t100\tx\n'
                             'node:btc:replay:TakerSellsLez\t1\tPASSED\t900\tx\n'
                             'node:btc:survivor:TakerSellsLez\t1\tPASSED\t500\tx\n')
        run, _ = self.regress('node:btc:happy:TakerSellsLez', 'node:btc:replay:TakerSellsLez',
                              'node:btc:survivor:TakerSellsLez', LEZ_LANES='2', LEZ_REGRESS_DURATIONS=str(durations),
                              plan={'happy': {'sleep': 0.1}, 'replay': {'sleep': 1}, 'survivor': {'sleep': 1}})
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        order = [line.split()[1].split(':')[0] for line in self.calls_of('start')]
        self.assertEqual(set(order[:2]), {'replay', 'survivor'})
        self.assertEqual(order[2], 'happy')

    def test_a_freeze_voids_every_lane_and_one_new_chain_serves_all(self):
        plan = {'happy': {'freeze_once': True, 'sleep': 0.5}, 'replay': {'sleep': 8}, 'survivor': {'sleep': 0.5}}
        run, _ = self.regress('node:btc:happy:TakerSellsLez', 'node:btc:replay:TakerSellsLez',
                              'node:btc:survivor:TakerSellsLez', plan=plan, LEZ_LANES='2',
                              LEZ_REGRESS_DURATIONS='/nonexistent')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        ups = self.calls_of('up')
        self.assertEqual(len(ups), 1, ups)
        self.assertIn('LEZ_LANES=2', ups[0])
        self.assertIn('--fresh-lez', ups[0])
        rows = self.results()
        frozen = [row for row in rows if row[2] == 'FROZEN']
        self.assertIn('node:btc:happy:TakerSellsLez', [row[0] for row in frozen])
        # every test passes in the end; the frozen ones on a second try, on the new chain
        passed = {row[0]: row for row in rows if row[2] == 'PASSED'}
        self.assertEqual(len(passed), 3)
        self.assertEqual(passed['node:btc:happy:TakerSellsLez'][1], '2')
        self.assertEqual(passed['node:btc:happy:TakerSellsLez'][6], '1', 'the retry runs on chain 1, the new one')
        # what was running beside the freezing test was void too
        voided = [row for row in frozen if row[0] != 'node:btc:happy:TakerSellsLez']
        self.assertEqual(len(voided), 1, rows)
        # each lane resets before its first test and before the third one, but
        # not after the new chain, which recreated its Nodes without swaps
        self.assertEqual(len(self.calls_of('reset')), 3)

    def test_a_failure_while_finality_moves_is_not_retried(self):
        run, _ = self.regress('node:btc:happy:TakerSellsLez', 'node:btc:replay:TakerSellsLez',
                              plan={'happy': {'rc': 1}}, LEZ_LANES='2')
        self.assertEqual(run.returncode, 1)
        rows = self.results()
        self.assertEqual([row[2] for row in rows if row[0] == 'node:btc:happy:TakerSellsLez'], ['FAILED'])
        self.assertEqual(self.calls_of('up'), [])
        self.assertIn('not passed: node:btc:happy:TakerSellsLez', run.stdout)

    def test_the_balance_refusal_runs_last_on_lane_one_alone(self):
        plan = {'happy': {'sleep': 1}, 'replay': {'sleep': 1}, 'zec-balance-refusal': {'sleep': 0.2}}
        run, _ = self.regress('node:zec:zec-balance-refusal:TakerSellsLez', 'node:btc:happy:TakerSellsLez',
                              'node:btc:replay:TakerSellsLez', plan=plan, LEZ_LANES='2')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        starts = self.calls_of('start')
        self.assertTrue(starts[-1].startswith('start zec-balance-refusal:TakerSellsLez lane=- '), starts)
        last_other_end = max(float(line.split('at=')[1]) for line in self.calls_of('end') if 'refusal' not in line)
        self.assertGreater(float(starts[-1].split('at=')[1]), last_other_end)
        self.assertTrue(self.calls_of('lanes')[-1].endswith('stop 2'))
        ups = self.calls_of('up')
        self.assertEqual(len(ups), 1)
        self.assertIn('LEZ_LANES=1 DEFER=taker-zurich-01', ups[0])


class LaneGroups(Runner):
    """The timing profile is a test's group: one profile at a time, Zcash tests
    in either, a switch (up.sh with LEZ_LANES) only with every lane idle."""
    def test_each_profile_runs_once_and_zcash_tests_run_in_either(self):
        plan = {'taker-refund': {'sleep': 1}, 'maker-refund': {'sleep': 1}, 'happy': {'sleep': 0.5},
                'replay': {'sleep': 0.5}, 'zec-happy': {'sleep': 0.5}}
        run, _ = self.regress('node:btc:happy:TakerSellsLez', 'node:btc:taker-refund:TakerSellsLez',
                              'node:zec:zec-happy:TakerSellsLez', 'node:btc:replay:TakerSellsLez',
                              'node:btc:maker-refund:TakerSellsLez', plan=plan, LEZ_LANES='2')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        ups = self.calls_of('up')
        # fast (runtime.env) -> refund (the longest first) -> fast: two switches, every lane's Nodes with them
        self.assertEqual(len(ups), 2, ups)
        self.assertTrue(all('LEZ_LANES=2' in line and '--fresh-lez' not in line for line in ups))
        starts = self.calls_of('start')
        refunds = [i for i, line in enumerate(starts) if 'refund' in line]
        fasts = [i for i, line in enumerate(starts) if line.split()[1] in ('happy:TakerSellsLez', 'replay:TakerSellsLez')]
        self.assertLess(max(refunds), min(fasts), starts)
        refund_ends = [float(line.split('at=')[1]) for line in self.calls_of('end') if 'refund' in line]
        fast_starts = [float(starts[i].split('at=')[1]) for i in fasts]
        self.assertLess(max(refund_ends), min(fast_starts), 'no refund test still ran at the switch')


class LaneNames(unittest.TestCase):
    def names(self, lane, **env):
        script = ('source "$1"; echo "$LEZ_CONTAINER_PREFIX $LEZ_LANE_RUNTIME $LEZ_ZEBRA_URL $LEZ_ZEBRA_HOST '
                  '$LEZ_MAKER_VNC_PORT $LEZ_TAKER_VNC_PORT $LEZ_LANE_MAKER_WALLET $LEZ_LANE_TAKER_WALLET '
                  '${LEZ_VOLUME_PREFIX:-unset} ${lane_compose[*]}"')
        clean = {k: v for k, v in os.environ.items() if not k.startswith('LEZ_')}
        return subprocess.run(['bash', '-c', script, '-', str(SCRIPTS / 'lane.sh')],
                              env={**clean, **({'LEZ_LANE': lane} if lane else {}), **env},
                              capture_output=True, text=True, check=True).stdout.split()

    def test_lane_one_is_the_stack_unchanged(self):
        self.assertEqual(self.names(None), ['lez', 'runtime', 'http://127.0.0.1:18232/', 'zebra', '5901', '5902',
                                            'maker-munich-01', 'taker-zurich-01', 'unset',
                                            '--env-file', 'runtime/runtime.env'])

    def test_lane_one_keeps_another_stacks_prefix(self):
        self.assertEqual(self.names('1', LEZ_CONTAINER_PREFIX='lez-testnet')[0], 'lez-testnet')

    def test_lane_two_has_its_own_names_and_the_second_market_wallets(self):
        self.assertEqual(self.names('2', LEZ_VOLUME_PREFIX='lez'),
                         ['lez-l2', 'runtime/lanes/2', 'http://127.0.0.1:18242/', 'lez-l2-zebra', '5911', '5912',
                          'maker-basel-02', 'taker-limmat-02', 'lez-l2',
                          '-p', 'lez-lane-2', '--env-file', 'runtime/runtime.env', '-f', 'compose.yaml',
                          '-f', 'compose.lane.yaml'])

    def test_a_lane_prefix_is_not_stacked_when_sourced_twice(self):
        clean = {k: v for k, v in os.environ.items() if not k.startswith('LEZ_')}
        out = subprocess.run(['bash', '-c', 'source "$1"; source "$1"; echo "$LEZ_VOLUME_PREFIX"', '-',
                              str(SCRIPTS / 'lane.sh')], env={**clean, 'LEZ_LANE': '3', 'LEZ_VOLUME_PREFIX': 'lez'},
                             capture_output=True, text=True, check=True).stdout.strip()
        self.assertEqual(out, 'lez-l3')

    def test_another_stack_derives_every_name_and_port_from_its_prefix_and_offset(self):
        stack = {'LEZ_STACK_PREFIX': 'lezb', 'LEZ_STACK_PORT_OFFSET': '1000'}
        self.assertEqual(self.names(None, **stack),
                         ['lezb', 'runtime', 'http://127.0.0.1:19232/', 'zebra', '6901', '6902',
                          'maker-munich-01', 'taker-zurich-01', 'unset',
                          '--env-file', 'runtime/runtime.env'])
        self.assertEqual(self.names('2', LEZ_VOLUME_PREFIX='lezb', **stack),
                         ['lezb-l2', 'runtime/lanes/2', 'http://127.0.0.1:19242/', 'lezb-l2-zebra', '6911', '6912',
                          'maker-basel-02', 'taker-limmat-02', 'lezb-l2',
                          '-p', 'lezb-lane-2', '--env-file', 'runtime/runtime.env', '-f', 'compose.yaml',
                          '-f', 'compose.lane.yaml'])
        self.assertEqual(self.stack(**stack), ['lezb', 'lezb-swap-stack', 'lezb-swap-chains', '19443', '4040',
                                               '9779', '4002', '4003', 'http://127.0.0.1:9779/'])

    def test_the_default_stack_keeps_its_names_and_ports(self):
        self.assertEqual(self.stack(), ['lez', 'lez-swap-stack', 'lez-swap-chains', '18443', '3040',
                                        '8779', '3002', '3003', 'http://127.0.0.1:8779/'])

    def stack(self, deploy=None, **env):
        script = ('source "$1" && echo "$LEZ_STACK_PREFIX $LEZ_STACK_PROJECT $LEZ_STACK_NETWORK $LEZ_BTC_RPC_PORT '
                  '$LEZ_SEQUENCER_PORT $LEZ_INDEXER_PORT $LEZ_BTC_EXPLORER_PORT $LEZ_LEZ_EXPLORER_PORT '
                  '$LEZ_STACK_INDEXER_URL"')
        clean = {k: v for k, v in os.environ.items() if not k.startswith('LEZ_')}
        lane = (deploy / 'scripts' / 'lane.sh') if deploy else (SCRIPTS / 'lane.sh')
        return subprocess.run(['bash', '-c', script, '-', str(lane)], env={**clean, **env},
                              capture_output=True, text=True, check=True).stdout.split()

    def test_a_deploy_directory_runs_the_stack_its_runtime_names(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        (tmp / 'scripts').mkdir()
        (tmp / 'runtime').mkdir()
        shutil.copy(SCRIPTS / 'lane.sh', tmp / 'scripts')
        (tmp / 'runtime' / 'runtime.env').write_text('LEZ_STACK_PREFIX=lezb\nLEZ_STACK_PORT_OFFSET=1000\n')
        self.assertEqual(self.stack(tmp)[:2], ['lezb', 'lezb-swap-stack'])
        with self.assertRaises(subprocess.CalledProcessError) as refused:
            self.stack(tmp, LEZ_STACK_PREFIX='lez')
        self.assertEqual(refused.exception.returncode, 64)
        self.assertIn("runs the stack 'lezb'", refused.exception.stderr)

    def test_not_a_stack_name_or_offset(self):
        for env in ({'LEZ_STACK_PREFIX': 'Lez_B'}, {'LEZ_STACK_PORT_OFFSET': '-5'}, {'LEZ_STACK_PORT_OFFSET': '50000'}):
            with self.assertRaises(subprocess.CalledProcessError, msg=env):
                self.stack(**env)

    def test_not_a_lane(self):
        run = subprocess.run(['bash', '-c', 'source "$1"', '-', str(SCRIPTS / 'lane.sh')],
                             env={**os.environ, 'LEZ_LANE': 'two'}, capture_output=True, text=True)
        self.assertEqual(run.returncode, 64)


if __name__ == '__main__':
    unittest.main()
