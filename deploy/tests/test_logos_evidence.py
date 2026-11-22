"""scripts/logos-evidence.py: the summary of one kept Logos phase of ui-e2e.sh logos-happy."""
import importlib.util
import pathlib
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'scripts' / 'logos-evidence.py'
spec = importlib.util.spec_from_file_location('logos_evidence', SCRIPT)
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


def line(stamp, module, text):
    return f'2026-11-22T{stamp}000000Z [2026-11-22 {stamp}] [out] [{module}] {text}'


INVITE, REQUEST = '0x' + 'a' * 64, '0x' + 'b' * 64
TAKER = [
    line('10:39:19.337', 'chat_module', 'INFO: chat_module::actions: delivery is online'),
    line('10:39:22.400', 'delivery_module', 'DBG calculateConnectionState tid=1 relayCount=1 storeClientCount=0'),
    line('10:39:22.571', 'chat_module', 'INFO: chat_module::actions: created direct conversation c0'),
    line('10:39:22.575', 'delivery_module', f'NTC start publish Waku message topics="waku relay" tid=1 msg_hash={INVITE} '
         'pubsubTopic=/waku/2/rs/7341/0 contentTopic=/logos-chat/1/inbox/proto'),
    line('10:39:22.576', 'delivery_module', f'INF Message propagated via Relay tid=1 msgHash={INVITE} noOfPeers=1'),
    line('10:39:22.611', 'chat_module', 'INFO: chat_module::actions: sent 7787 bytes to c0'),
    line('10:39:22.613', 'delivery_module', f'NTC start publish Waku message topics="waku relay" tid=1 msg_hash={REQUEST} '
         'pubsubTopic=/waku/2/rs/7341/0 contentTopic=/logos-chat/1/conv/proto'),
    line('10:39:22.614', 'delivery_module', f'INF Message propagated via Relay tid=1 msgHash={REQUEST} noOfPeers=1'),
]
MAKER = [
    line('10:39:06.627', 'delivery_module', 'INF Configuration: Enabled protocols topics="waku conf" tid=1 relay=true store=false'),
    line('10:39:10.654', 'chat_module', 'INFO: chat_module::actions: delivery is online'),
    line('10:39:10.681', 'logos', 'DeliveryModuleImpl::subscribe called with contentTopic: /logos-chat/1/inbox/proto'),
    line('10:39:22.400', 'delivery_module', 'DBG calculateConnectionState tid=1 relayCount=1 storeClientCount=0'),
    line('10:39:22.577', 'delivery_module', f'DBG received relay message topics="waku relay" tid=1 msg_hash={INVITE} msg_id=x'),
    line('10:39:22.578', 'logos', f'DeliveryModuleImpl::event_callback message: {{"eventType":"message_received","messageHash":"{INVITE}"}}'),
    line('10:39:22.621', 'delivery_module', f'DBG received relay message topics="waku relay" tid=1 msg_hash={REQUEST} msg_id=y'),
    line('10:39:22.631', 'logos', 'DeliveryModuleImpl::subscribe called with contentTopic: /logos-chat/1/conv/proto'),
]


class Summary(unittest.TestCase):
    def test_a_request_before_the_subscription_is_a_negative_margin(self):
        with tempfile.TemporaryDirectory() as root:
            directory = pathlib.Path(root)
            (directory / 'taker-container.log').write_text('\n'.join(TAKER) + '\n')
            (directory / 'maker-container.log').write_text('\n'.join(MAKER) + '\n')
            out = evidence.summary(directory)
        text = '\n'.join(out)
        self.assertIn('first request 10:39:22.611, 7787 B: 3.3 s after the Taker came online', text)
        self.assertIn('relay peers then: Taker 1, Maker 1', text)
        self.assertIn('the Taker sent 1 message(s), the Maker received 0', text)
        self.assertIn(f'{REQUEST[:14]}: relayed to 1 peer(s); Maker relay 10:39:22.621, handed to its Chat module no', text)
        self.assertIn(f'{INVITE[:14]}: relayed to 1 peer(s); Maker relay 10:39:22.577, handed to its Chat module 10:39:22.578', text)
        self.assertIn('margin -0.010 s', text)
        self.assertIn('Maker protocols: relay=true store=false', text)

    def test_a_missing_directory_summarises_nothing_sent(self):
        with tempfile.TemporaryDirectory() as root:
            out = evidence.summary(pathlib.Path(root) / 'absent')
        self.assertIn('the Taker sent 0 message(s), the Maker received 0', '\n'.join(out))


if __name__ == '__main__':
    unittest.main()
