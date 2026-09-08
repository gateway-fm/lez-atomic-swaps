#!/usr/bin/env python3
"""Record reviewer scenarios and verify their public chain effects; Python stdlib only."""
import argparse
import contextlib
import datetime
import hashlib
import importlib.util
import json
import pathlib
import subprocess
import sys
import threading
import time
import traceback

DEPLOY = pathlib.Path(__file__).resolve().parent.parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, DEPLOY/'scripts'/filename)
    obj = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(obj)
    return obj


E = module('scenario_harness', 'node-e2e.py')
X = module('public_evidence', 'export-node-evidence.py')


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def collect(name, summary, out, commit):
    ids = summary.get('swap_ids', [summary.get('swap_id')])
    for sid in filter(None, ids):
        view = E.taker_view(sid)
        aggregate = X.taker_aggregate(sid)
        snapshot = aggregate['snapshot']
        reader = '''import json,sqlite3,sys
s=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
rows=[]
for kind,payload in s.execute('select evidence_kind,payload_json from btc_actor_evidence order by aggregate_revision'):
 d=json.loads(payload); rows.append({'kind':kind,'chain':d['chain'],'proof':d['proof'],'refund_position':d.get('refund_position')})
print(json.dumps(rows))'''
        journal = json.loads(X.docker('lez-taker-node','python3','-c',reader,
            X.TAKER_SWAPS+'/'+aggregate['directory']+'/actor/state.sqlite3'))
        public = {k:v for k,v in snapshot.items() if k.endswith('transaction_id') or k in ('phase', 'direction')}
        for item in journal:
            if item['kind']=='taker_refund': public['taker_refund_event_transaction_id']=item['proof']['transaction_id']
            if item['kind']=='maker_refund': public['maker_recovery_transaction_id']=item['proof']['transaction_id']
        evidence = {'public_evidence_journal':journal, 'repository_commit': commit, 'scenario': name, 'swap_id': sid,
                    'taker': view, 'maker': E.maker_view(sid), 'aggregate_revision': aggregate['revision'],
                    'public_aggregate': public, 'bitcoin_transactions': {}, 'lez_transactions': {}}
        for key in ('taker_lock_transaction_id', 'followup_claim_transaction_id', 'taker_refund_event_transaction_id'):
            txid = public.get(key)
            if txid:
                tx, height, block = X.bitcoin_facts(txid)
                evidence['bitcoin_transactions'][key] = {'txid':txid, 'height':height, 'block':block,
                    'confirmations':tx['confirmations'], 'vin':[{k:v for k,v in i.items() if k in ('txid','vout','sequence')} for i in tx['vin']],
                    'vout':tx['vout']}
                if tx['confirmations'] < 1: raise RuntimeError('unconfirmed Bitcoin effect')
        btc = evidence['bitcoin_transactions']
        if btc:
            first = E.bitcoin('getblockheader', min(btc.values(),key=lambda t:t['height'])['block'])['time']
            index = X.LezIndex(first, int(time.time()))
            for key in ('maker_lock_transaction_id','revealing_claim_transaction_id','maker_recovery_transaction_id'):
                txid = public.get(key)
                if txid:
                    height, block, status = index.facts(txid)
                    evidence['lez_transactions'][key] = {'txid':txid,'height':height,'block':block,'status':status}
                    if status != 'Finalized': raise RuntimeError('LEZ effect not finalized')
        expected_btc = {'taker_lock_transaction_id', 'followup_claim_transaction_id'} if name == 'concurrent' else {'taker_lock_transaction_id', 'taker_refund_event_transaction_id'}
        expected_lez = {'maker_lock_transaction_id', 'revealing_claim_transaction_id'} if name == 'concurrent' else ({'maker_lock_transaction_id', 'maker_recovery_transaction_id'} if name == 'maker-refund' else set())
        if set(btc) != expected_btc or set(evidence['lez_transactions']) != expected_lez:
            raise RuntimeError('missing or unexpected public chain effects')
        refund = btc.get('taker_refund_event_transaction_id')
        lock = btc.get('taker_lock_transaction_id')
        if refund and lock:
            spent = [i['vout'] for i in refund['vin'] if i.get('txid') == lock['txid']]
            if not spent: raise RuntimeError('refund does not spend recorded lock')
            principal_sats = sum(round(lock['vout'][i]['value']*1e8) for i in spent)
            returned_sats = sum(round(v['value']*1e8) for v in refund['vout'])
            evidence['bitcoin_refund_reconciliation'] = {'spent_lock_outputs':spent,'principal_sats':principal_sats,
                'refund_outputs_sats':returned_sats,'fee_sats':principal_sats-returned_sats,
                'note':'On-chain output reconciliation; Core wallet balance alone is not a refund proof.'}
        if refund and lock:
            contribution = json.loads(X.docker('lez-taker-node', 'cat',
                X.TAKER_SWAPS+'/'+aggregate['directory']+'/role/contribution-summary.json'))
            expected = contribution['bitcoin_claim_destination_script_pubkey']
            if isinstance(expected, list):
                expected = bytes(expected).hex()
            if len(refund['vout']) != 1 or refund['vout'][0]['scriptPubKey']['hex'] != expected:
                raise RuntimeError('refund destination differs from Taker contribution')
            check = evidence['bitcoin_refund_reconciliation']
            if (check['principal_sats'], check['refund_outputs_sats'], check['fee_sats']) != (E.FOREIGN_UNITS, E.FOREIGN_UNITS-1000, 1000):
                raise RuntimeError('unexpected refund principal or fee')
            check['destination_script_matches_taker_contribution'] = True
            check['expected_script_pubkey'] = expected
        if name == 'maker-refund':
            identity = json.loads((E.RUNTIME/'lez/maker/identity.json').read_text())
            account = identity['account_id']
            fund = evidence['lez_transactions']['maker_lock_transaction_id']['height']
            recovery = evidence['lez_transactions']['maker_recovery_transaction_id']['height']
            balances = {}
            for label, height in [('before_funding', fund-1), ('after_funding', fund),
                                  ('before_refund', recovery-1), ('after_refund', recovery)]:
                balances[label] = {'height': height, 'balance': X.indexer('getAccountAtBlock', [account, height])['balance']}
            funded = balances['before_funding']['balance'] - balances['after_funding']['balance']
            returned = balances['after_refund']['balance'] - balances['before_refund']['balance']
            if funded != E.LEZ_UNITS or returned != funded:
                raise RuntimeError('historical LEZ funding/refund balance mismatch')
            evidence['lez_refund_reconciliation'] = {'account_id': account, 'historical_balances': balances,
                                                     'funded_units': funded, 'returned_units': returned}
        (out/f'{sid}.json').write_text(json.dumps(evidence,indent=2)+'\n')
        if view['state']=='completed':
            full = X.build_evidence(view, commit)
            (out/f'{sid}-five-effects.json').write_text(json.dumps(full,indent=2)+'\n')
        print(f'  Verified public chain evidence for {sid}',flush=True)


class Tee:
    def __init__(self, log, cast, started):
        self.log, self.cast, self.started = log, cast, started
        self.lock = threading.Lock()

    def write(self, text):
        with self.lock:
            self.log.write(text)
            self.log.flush()
            self.cast.write(json.dumps([round(time.monotonic()-self.started, 3), 'o', text.replace('\n', '\r\n')])+'\n')
            self.cast.flush()
            sys.__stdout__.write(text)
            sys.__stdout__.flush()
        return len(text)

    def flush(self):
        self.log.flush()
        self.cast.flush()


def record(name, root, commit):
    out = root/name
    out.mkdir()
    started = time.monotonic()
    stopped = threading.Event()
    ok = False
    with (out/'execution.log').open('w') as log, (out/'execution.cast').open('w') as cast:
        cast.write(json.dumps({'version': 2, 'width': 158, 'height': 48,
                               'timestamp': int(time.time()), 'title': name+' / '+commit})+'\n')
        tee = Tee(log, cast, started)

        def heartbeat():
            while not stopped.wait(30):
                tee.write(f'[{datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")}] Running; elapsed {int(time.monotonic()-started)} seconds\n')

        thread = threading.Thread(target=heartbeat, daemon=True)
        try:
            with contextlib.redirect_stdout(tee), contextlib.redirect_stderr(tee):
                print(f'LEZ atomic swaps — Node owner-API execution\nScenario: {name}\nSource: {commit}')
                print('Bitcoin regtest + LEZ devnet; fast timing. This is an API recording.\n')
                thread.start()
                ok = E.run(name)
                summary = json.loads((E.RUNTIME/'e2e'/f'{name}.json').read_text())
                (out/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
                if ok:
                    collect(name, summary, out, commit)
                    print('PASS — scenario and public chain evidence verified.')
        except BaseException:
            ok = False
            tee.write(traceback.format_exc())
        finally:
            stopped.set()
            if thread.ident is not None:
                thread.join()
    (out/'recording-result.json').write_text(json.dumps({'scenario': name, 'passed_including_evidence': ok,
        'elapsed_seconds': round(time.monotonic()-started), 'repository_commit': commit}, indent=2)+'\n')
    return ok


def provenance(commit):
    binaries = {}
    for role in ('maker', 'taker'):
        paths = [f'lez-{role}-node', f'lez-btc-{role}-actor', 'lez-v02-bridge-poc']
        for binary in paths:
            staged = DEPLOY/'images'/f'{role}-node'/binary
            expected = hashlib.sha256(staged.read_bytes()).hexdigest()
            actual = command('docker', 'exec', f'lez-{role}-node', 'sha256sum', '/usr/local/bin/'+binary).split()[0]
            if expected != actual:
                raise RuntimeError(f'{role}/{binary}: running binary differs from staged build; recreate the Nodes')
            binaries[f'{role}/{binary}'] = actual
    for receipt in (DEPLOY/'images/maker-node/build-source.txt', DEPLOY/'images/maker-node/sidecar-source.txt'):
        if receipt.read_text().strip() != commit:
            raise RuntimeError(f'{receipt.name}: rebuild this checkout with from-scratch.sh')
    containers = {}
    for name in ('lez-bitcoin-core', 'lez-bedrock', 'lez-sequencer', 'lez-indexer', 'lez-maker-node', 'lez-taker-node'):
        containers[name] = command('docker', 'inspect', name, '--format', '{{.Image}}')
    return {'repository_commit': commit, 'timing': E.timing_profile(), 'binary_sha256': binaries,
            'container_images': containers, 'bitcoin_genesis': E.bitcoin('getblockhash', '0'),
            'recorded_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'scope': 'BTC to LEZ on local regtest/devnet; API scenarios, not UI or mainnet evidence'}


def finalize(root):
    # No runtime directories, wallet databases, env files or private identities
    # are copied. Only the explicitly written public artifacts are packaged.
    template = (DEPLOY/'scripts/reviewer-player.html').read_text()
    sessions = {}
    for cast in sorted(root.glob('*/execution.cast')):
        sessions[cast.parent.name] = [json.loads(line) for line in cast.read_text().splitlines()]
    data = json.dumps(sessions).replace('<', '\\u003c')
    (root/'index.html').write_text(template.replace('/* RECORDINGS */ {}', data))
    entries = []
    for path in sorted(root.rglob('*')):
        if path.is_file() and path.name != 'SHA256SUMS':
            entries.append(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(root).as_posix()}\n')
    (root/'SHA256SUMS').write_text(''.join(entries))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, help='new output directory; existing paths are refused')
    parser.add_argument('--scenario', choices=['concurrent', 'taker-refund', 'maker-refund', 'all'], default='all')
    args = parser.parse_args()
    repo = DEPLOY.parent
    commit = command('git', '-C', str(repo), 'rev-parse', 'HEAD')
    if command('git', '-C', str(repo), 'status', '--porcelain', '--untracked-files=no'):
        parser.error('commit tracked changes before recording so the source archive identifies this run exactly')
    if E.timing_profile().get('LEZ_TIMING_PROFILE') != 'fast':
        parser.error('use from-scratch.sh --reviewer to prepare the fast timing profile')
    if E.bitcoin('getblockchaininfo')['chain'] != 'regtest':
        parser.error('Bitcoin must be regtest')
    metadata = provenance(commit)
    root = (args.output or DEPLOY/'runtime/recordings'/datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%d-%H%M%S')).resolve()
    root.mkdir(parents=True, exist_ok=False)
    (root/'provenance.json').write_text(json.dumps(metadata, indent=2)+'\n')
    subprocess.run(['git', '-C', str(repo), 'archive', '--format=tar.gz', '-o', str(root/'source.tar.gz'), commit], check=True)
    names = ['concurrent', 'taker-refund', 'maker-refund'] if args.scenario == 'all' else [args.scenario]
    ok = True
    try:
        for name in names:
            if not record(name, root, commit):
                ok = False
                break
        if provenance(commit)['binary_sha256'] != metadata['binary_sha256']:
            raise RuntimeError('binaries changed during recording')
    except BaseException:
        ok = False
        (root/'failure.txt').write_text(traceback.format_exc())
    finally:
        (root/'result.json').write_text(json.dumps({'passed': ok, 'requested_scenarios': names}, indent=2)+'\n')
        finalize(root)
    print(f'Evidence: {root}\nReplay: {root / "index.html"}')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
