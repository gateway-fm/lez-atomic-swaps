"""The role Node's launcher (modules/lez-role-node/node-entrypoint.sh) on Linux
and on macOS.

On Linux it keeps the container's paths and forwards loopback ports to the
Compose services by name, each forward under `setpriv --pdeathsig` and a
child of the Node itself. On macOS (uname Darwin) it keeps everything under
~/Library/Application Support/LEZ, reaches the chains on loopback directly
(no socat), and runs under macOS's /bin/bash 3.2 without setpriv. The Node,
its CLI, curl, socat, setpriv and uname are fakes; nothing here needs a chain.
"""
import json
import os
import pathlib
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unittest

LAUNCHER = pathlib.Path(__file__).resolve().parents[2] / 'modules/lez-role-node/node-entrypoint.sh'
PROGRAM = 'c' * 64
GENESIS = 'd' * 64
CHANNEL = 'e' * 64
ACCOUNT = 'f' * 64
MAKER_KEY = '02' + 'ab' * 32
# What the launcher needs from the host besides the fakes.
TOOLS = ('env', 'dirname', 'cat', 'mkdir', 'chmod', 'rm', 'install', 'jq', 'sed', 'head', 'cut',
         'tr', 'grep', 'od', 'cmp', 'mv', 'seq', 'sleep', 'wc', 'tee', 'python3', 'sha256sum',
         'shasum', 'ls', 'true')


def executable(path, text):
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def bashes():
    """Every bash this host has that the launcher must run under: the one on
    PATH and macOS's /bin/bash 3.2."""
    found = [shutil.which('bash')]
    if os.path.exists('/bin/bash') and os.path.realpath('/bin/bash') != os.path.realpath(found[0]):
        found.append('/bin/bash')
    return found


@unittest.skipUnless(shutil.which('jq'), 'the launcher renders its configuration with jq')
class Launcher(unittest.TestCase):
    def setUp(self):
        # A short root: macOS gives TMPDIR under /var/folders/…, and a Unix
        # socket path below it is longer than the launcher (and the kernel) allows.
        self.tmp = pathlib.Path(tempfile.mkdtemp(dir='/tmp' if sys.platform == 'darwin' else None))
        self.addCleanup(shutil.rmtree, self.tmp)
        self.calls = self.tmp / 'calls.log'
        self.record = self.tmp / 'node.json'
        # The package's bin/: the launcher beside fakes of what it runs.
        self.payload = self.tmp / 'payload'
        self.payload.mkdir()
        shutil.copy(LAUNCHER, self.payload / 'node-entrypoint.sh')
        # The Node records its pid, its path and its arguments, one per line.
        node = '#!/bin/sh\n{ echo "$$"; echo "$0"; for a; do printf "%s\\n" "$a"; done; } >"$RECORD"\n'
        for role in ('maker', 'taker'):
            executable(self.payload / f'lez-{role}-node', node)
            executable(self.payload / f'lez-btc-{role}-actor', '#!/bin/sh\nexit 0\n')
        executable(self.payload / 'lez-maker-cli', f'#!/bin/sh\necho "{MAKER_KEY}"\n')
        executable(self.payload / 'lez-taker-registry-init', '#!/bin/sh\n: >"$2"\n')
        executable(self.payload / 'lez-v02-bridge-poc', '#!/bin/sh\nexit 0\n')
        # The host: only these tools, so a host without setpriv is one.
        self.tools = self.tmp / 'tools'
        self.tools.mkdir()
        for tool in TOOLS:
            real = shutil.which(tool)
            if real:
                (self.tools / tool).symlink_to(real)
        self.fakes = self.tmp / 'fakes'
        self.fakes.mkdir()
        executable(self.fakes / 'curl', '#!/bin/sh\n'
                   'for a in "$@"; do case "$a" in\n'
                   '  *getblockhash*) echo \'{"result":"0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206"}\'; exit 0;;\n'
                   '  *getnewaddress*) echo \'{"result":"bcrt1pclaim"}\'; exit 0;;\n'
                   '  *getblockcount*) echo \'{"result":101}\'; exit 0;;\n'
                   'esac; done\nexit 22\n')
        executable(self.fakes / 'socat', '#!/bin/sh\necho "socat $*" >>"$CALLS"\n')

    def host(self, system, setpriv):
        executable(self.fakes / 'uname', f'#!/bin/sh\necho {system}\n')
        if setpriv:
            executable(self.fakes / 'setpriv',
                       '#!/bin/sh\necho "setpriv ppid=$PPID $*" >>"$CALLS"\nshift 2\nexec "$@"\n')

    def inputs(self, identity, manifest, cookie, role='maker'):
        identity.mkdir(parents=True)
        (identity / 'identity.json').write_text(json.dumps({'account_id_hex': ACCOUNT}))
        (identity / 'lez-signer.key').write_text('11' * 32)
        if role == 'maker':
            (identity / 'zcash.key').write_text('22' * 32)
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(f'M3_POC_LEZ_ESCROW_PROGRAM_ID={PROGRAM}\nM3_POC_LEZ_GENESIS_BLOCK_HASH={GENESIS}\n')
        cookie.parent.mkdir(parents=True, exist_ok=True)
        cookie.write_text('lezrpc:secret')

    def run_launcher(self, bash, role, **env):
        environment = {
            'PATH': f'{self.fakes}:{self.tools}', 'HOME': str(self.tmp / 'home'),
            'CALLS': str(self.calls), 'RECORD': str(self.record),
            'LEZ_SIDECAR_PORT_BASE': '19000', 'LEZ_V02_CHANNEL_ID': CHANNEL, **env,
        }
        result = subprocess.run([bash, str(self.payload / 'node-entrypoint.sh'), role], env=environment,
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        pid, program, *args = self.record.read_text().splitlines()
        return {'pid': pid, 'program': program, 'args': args}

    def logged(self, lines):
        # The forwards run in the background and may log after the Node exits.
        for _ in range(50):
            text = self.calls.read_text() if self.calls.exists() else ''
            if text.count('\n') >= lines:
                return text
            time.sleep(0.1)
        return text

    def flag(self, node, name):
        return node['args'][node['args'].index(name) + 1]

    def test_linux_forwards_each_chain_as_a_child_of_the_node(self):
        self.host('Linux', setpriv=True)
        root = self.tmp / 'linux'
        self.inputs(root / 'identity', root / 'market/market-bootstrap.env', root / 'config/btc-rpc-cookie')
        node = self.run_launcher(
            shutil.which('bash'), 'maker',
            LEZ_NODE_STATE_DIR=str(root / 'state'), LEZ_NODE_RUN_ROOT=str(root / 'run'),
            LEZ_ZEC_STATE_DIR=str(root / 'zec'), LEZ_NODE_IDENTITY_DIR=str(root / 'identity'),
            LEZ_MARKET_MANIFEST=str(root / 'market/market-bootstrap.env'),
            LEZ_BTC_RPC_COOKIE_FILE=str(root / 'config/btc-rpc-cookie'), LEZ_DELIVERY_DIR=str(root / 'delivery'))
        calls = self.logged(8).splitlines()
        for port, host in (('18443', 'bitcoin-core'), ('3040', 'sequencer'), ('8779', 'indexer'), ('18232', 'zebra')):
            forward = f'socat TCP-LISTEN:{port},bind=127.0.0.1,fork,reuseaddr TCP:{host}:{port}'
            # The forward is the Node's own child (the launcher execs the
            # Node), so --pdeathsig fires when the Node goes.
            self.assertIn(f'setpriv ppid={node["pid"]} --pdeathsig TERM {forward}', calls)
            self.assertIn(forward, calls)
        self.assertEqual(self.flag(node, '--socket'), str(root / 'run/maker/node.sock'))
        self.assertEqual(self.flag(node, '--delivery-directory'), str(root / 'delivery'))
        role = json.loads((root / 'state/btc/btc-role.json').read_text())
        self.assertEqual(role['bitcoin']['endpoint'], 'http://127.0.0.1:18443/')
        self.assertEqual(role['lez']['sequencer_url'], 'http://sequencer:3040')
        self.assertEqual(role['lez']['indexer_url'], 'http://indexer:8779')
        self.assertEqual(role['actor']['program'], str(self.payload / 'lez-btc-maker-actor'))
        self.assertEqual((root / 'delivery/maker-delivery-identity.pub').read_text(), MAKER_KEY + '\n')

    def test_linux_defaults_are_the_container_mounts(self):
        text = LAUNCHER.read_text()
        linux = text[text.index('\nelse\n  default_state='):text.index('\nfi\n', text.index('\nelse\n  default_state='))]
        for line in ('default_state="/var/lib/lez/$role"', 'default_run_root=/run/lez',
                     'default_zec_state=/var/lib/lez/zec', 'default_zec_sidecar=\n', 'default_identity=/run-lez-identity',
                     'default_manifest=/run-market/market-bootstrap.env',
                     'default_cookie=/run-config/btc-rpc-cookie', 'default_delivery=/delivery',
                     'default_chain_host=\n'):
            self.assertIn(line, linux + '\n')

    def test_a_service_on_loopback_is_reached_without_a_forward(self):
        self.host('Linux', setpriv=False)
        root = self.tmp / 'linux'
        self.inputs(root / 'identity', root / 'manifest.env', root / 'cookie')
        self.run_launcher(
            shutil.which('bash'), 'maker',
            LEZ_NODE_STATE_DIR=str(root / 'state'), LEZ_NODE_RUN_ROOT=str(root / 'run'),
            LEZ_ZEC_STATE_DIR=str(root / 'zec'), LEZ_NODE_IDENTITY_DIR=str(root / 'identity'),
            LEZ_MARKET_MANIFEST=str(root / 'manifest.env'), LEZ_BTC_RPC_COOKIE_FILE=str(root / 'cookie'),
            LEZ_DELIVERY_DIR=str(root / 'delivery'), LEZ_BITCOIN_CORE_HOST='127.0.0.1',
            LEZ_SEQUENCER_HOST='localhost')
        calls = self.logged(2)
        self.assertNotIn('TCP:127.0.0.1', calls)
        self.assertNotIn('TCP:localhost', calls)
        # Without setpriv the forwards still start, directly.
        self.assertIn('socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr TCP:indexer:8779', calls)
        self.assertIn('socat TCP-LISTEN:18232,bind=127.0.0.1,fork,reuseaddr TCP:zebra:18232', calls)
        self.assertNotIn('setpriv', calls)

    def test_macos_keeps_both_roles_under_application_support(self):
        self.host('Darwin', setpriv=False)
        home = self.tmp / 'home/Library/Application Support/LEZ'
        self.inputs(home / 'identity/maker', home / 'market-bootstrap.env', home / 'btc-rpc-cookie')
        (home / 'identity/taker').mkdir(parents=True)
        (home / 'identity/taker/identity.json').write_text(json.dumps({'account_id_hex': ACCOUNT}))
        (home / 'identity/taker/lez-signer.key').write_text('33' * 32)
        for bash in bashes():
            with self.subTest(bash=bash):
                maker = self.run_launcher(bash, 'maker')
                self.assertEqual(maker['program'], str(self.payload / 'lez-maker-node'))
                self.assertEqual(self.flag(maker, '--socket'), str(home / 'run/maker/node.sock'))
                self.assertEqual(self.flag(maker, '--chat-socket'), str(home / 'run/maker/chat.sock'))
                self.assertEqual(self.flag(maker, '--database'), str(home / 'maker/maker.sqlite3'))
                self.assertEqual(self.flag(maker, '--delivery-directory'), str(home / 'delivery'))
                # The minted Zcash key is the Maker's Delivery signing key.
                self.assertEqual((home / 'maker/delivery-signing.key').read_text(), '22' * 32)
                role = json.loads((home / 'maker/btc/btc-role.json').read_text())
                self.assertEqual(role['lez']['sequencer_url'], 'http://127.0.0.1:3040')
                self.assertEqual(role['lez']['indexer_url'], 'http://127.0.0.1:8779')
                self.assertEqual(role['bitcoin']['cookie_file'], str(home / 'maker/btc/btc-rpc-cookie'))
                self.assertEqual(stat.S_IMODE((home / 'maker').stat().st_mode), 0o700)

                taker = self.run_launcher(bash, 'taker')
                self.assertEqual(self.flag(taker, '--socket'), str(home / 'run/taker/node.sock'))
                config = json.loads((home / 'taker/role.json').read_text())
                self.assertEqual(config['delivery_sources'][0]['directory'], str(home / 'delivery'))
                self.assertEqual(config['delivery_sources'][0]['maker_public_key'], MAKER_KEY)
                self.assertEqual(config['chat_socket'], str(home / 'run/maker/chat.sock'))
                self.assertEqual(config['initiation']['btc_role_config'], str(home / 'taker/btc/btc-role.json'))
                # No forwards: the chains' ports are on this host's loopback.
                self.assertFalse(self.calls.exists(), self.calls.read_text() if self.calls.exists() else '')

    def test_one_environment_starts_both_roles_with_their_own_settings(self):
        # One host (one Basecamp with both desks) has one environment: each
        # role's own LEZ_<ROLE>_<NAME> wins over the shared LEZ_<NAME>.
        self.host('Darwin', setpriv=False)
        home = self.tmp / 'home/Library/Application Support/LEZ'
        self.inputs(home / 'identity/maker', home / 'market-bootstrap.env', home / 'btc-rpc-cookie')
        (home / 'identity/taker').mkdir(parents=True)
        (home / 'identity/taker/identity.json').write_text(json.dumps({'account_id_hex': ACCOUNT}))
        sockets = []
        for role in ('maker', 'taker'):
            path = self.tmp / f'{role}-signer.sock'
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(path))
            self.addCleanup(listener.close)
            sockets.append(path)
        env = {
            'LEZ_SIDECAR_PORT_BASE': '1', 'LEZ_BTC_WALLET': 'shared', 'LEZ_NODE_LOG_FILE': str(self.tmp / 'shared.log'),
            'LEZ_MAKER_SIDECAR_PORT_BASE': '19000', 'LEZ_TAKER_SIDECAR_PORT_BASE': '19500',
            'LEZ_MAKER_WALLET_SIGNER_SOCKET': str(sockets[0]), 'LEZ_TAKER_WALLET_SIGNER_SOCKET': str(sockets[1]),
            'LEZ_MAKER_BTC_WALLET': 'lez-maker', 'LEZ_TAKER_BTC_WALLET': 'lez-taker',
            'LEZ_MAKER_NODE_LOG_FILE': str(self.tmp / 'maker.log'), 'LEZ_TAKER_NODE_LOG_FILE': str(self.tmp / 'taker.log'),
            'LEZ_TAKER_BTC_REFUND_CSV_BLOCKS': '99',
        }
        for bash in bashes():
            with self.subTest(bash=bash):
                for role, port, sock, wallet in (('maker', 19000, sockets[0], 'lez-maker'),
                                                 ('taker', 19500, sockets[1], 'lez-taker')):
                    self.run_launcher(bash, role, **env)
                    config = json.loads((home / f'{role}/btc/btc-role.json').read_text())
                    self.assertEqual(config['lez']['sidecar_port_base'], port)
                    self.assertEqual(config['lez']['wallet_signer']['socket'], str(sock))
                    self.assertIsNone(config['lez']['signer_key_file'])
                    self.assertEqual(config['bitcoin']['wallet'], wallet)
                    # Unset for the role: the shared name (or the default).
                    self.assertEqual(config['bitcoin']['refund_csv_blocks'], 99 if role == 'taker' else 60)
                    self.assertTrue((self.tmp / f'{role}.log').exists())
                self.assertFalse((self.tmp / 'shared.log').exists())

    def test_a_socket_directory_too_long_for_a_unix_socket_is_refused(self):
        self.host('Darwin', setpriv=False)
        result = subprocess.run(
            [shutil.which('bash'), str(self.payload / 'node-entrypoint.sh'), 'maker'],
            env={'PATH': f'{self.fakes}:{self.tools}', 'HOME': str(self.tmp), 'LEZ_SIDECAR_PORT_BASE': '19000',
                 'LEZ_V02_CHANNEL_ID': CHANNEL, 'LEZ_NODE_RUN_ROOT': '/' + 'r' * 100},
            capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 2)
        self.assertIn('too long for a Unix socket path', result.stderr)


if __name__ == '__main__':
    unittest.main()
