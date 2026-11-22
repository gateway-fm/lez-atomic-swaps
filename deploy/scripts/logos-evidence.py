#!/usr/bin/env python3
"""logos-evidence.py <dir>...: what one Logos phase of ui-e2e.sh logos-happy shows.

<dir> is what ui-e2e.sh keeps under <lane runtime>/evidence/logos/: each role's
container output since the switch to the Logos transport (maker-container.log,
taker-container.log). Printed per directory:

- when each side's Chat module came online and when the Taker created the
  conversation and sent its first request (its size, and each side's relay
  peer count then, from calculateConnectionState);
- every Taker publish (content topic, relay outcome), when the Maker's relay
  received it, and whether the Maker's Delivery node handed it to its Chat
  module (message_received);
- when the Maker subscribed to the conversation's content topic, against the
  first request's arrival at its relay: a negative margin is a request that
  arrived before the subscription, which the Maker's Delivery node does not
  hand on.

The Delivery nodes of the devnet's Logos transport print the relay's
per-message lines (DBG/INF) although compose.logos.yaml asks for log-level
ERROR; without them only the Chat lines are summarised.
"""
import pathlib
import re
import sys


def read(path: pathlib.Path) -> list[str]:
    try:
        return path.read_text(errors="replace").splitlines()
    except FileNotFoundError:
        return []


def when(line: str) -> str:
    """The time of a module's line (HH:MM:SS.mmm, UTC), from its own stamp."""
    found = re.search(r"\[\d{4}-\d\d-\d\d (\d\d:\d\d:\d\d\.\d+)\]", line)
    if found:
        return found.group(1)
    found = re.match(r"\d{4}-\d\d-\d\dT(\d\d:\d\d:\d\d\.\d+)", line)
    return found.group(1) if found else "?"


def seconds(stamp: str) -> float:
    hours, minutes, rest = stamp.split(":")
    return int(hours) * 3600 + int(minutes) * 60 + float(rest)


def relay_peers(lines: list[str], before: str) -> str:
    count = "?"
    for line in lines:
        found = re.search(r"calculateConnectionState .*relayCount=(\d+)", line)
        if found and seconds(when(line)) <= seconds(before):
            count = found.group(1)
    return count


def summary(directory: pathlib.Path) -> list[str]:
    taker, maker = read(directory / "taker-container.log"), read(directory / "maker-container.log")
    chat = lambda lines, text: [l for l in lines if "[chat_module]" in l and text in l]
    online = {role: next((when(l) for l in chat(lines, "delivery is online")), None)
              for role, lines in (("maker", maker), ("taker", taker))}
    created = next((when(l) for l in chat(taker, "created direct conversation")), None)
    sent = [(when(l), re.search(r"sent (\d+) bytes", l).group(1)) for l in taker
            if "[chat_module]" in l and re.search(r"sent \d+ bytes", l)]
    received = [when(l) for l in maker if "[chat_module]" in l and re.search(r"received \d+ bytes", l)]
    publishes, outcome = [], {}
    for line in taker:
        found = re.search(r"start publish Waku message .*msg_hash=(0x[0-9a-f]+) pubsubTopic=(\S+) contentTopic=(\S+)", line)
        if found:
            publishes.append((when(line), found.group(1), found.group(3)))
        found = re.search(r"Message propagated via Relay .*msgHash=(0x[0-9a-f]+) noOfPeers=(\d+)", line)
        if found:
            outcome[found.group(1)] = f"relayed to {found.group(2)} peer(s)"
        found = re.search(r"Failed to publish message with relay .*msgHash=(0x[0-9a-f]+) error=\"([^\"]+)\"", line)
        if found:
            outcome.setdefault(found.group(1), f"relay failed: {found.group(2)}")
    at_relay, handed_on = {}, {}
    for line in maker:
        found = re.search(r"received relay message .*msg_hash=(0x[0-9a-f]+)", line)
        if found:
            at_relay.setdefault(found.group(1), when(line))
        found = re.search(r'"eventType":"message_received","messageHash":"(0x[0-9a-f]+)"', line)
        if found:
            handed_on.setdefault(found.group(1), when(line))
    subscriptions = [(when(l), l.split("contentTopic: ", 1)[1].strip()) for l in maker
                     if "DeliveryModuleImpl::subscribe called with contentTopic: /logos-chat/" in l]
    out = [f"online: Maker {online['maker']}, Taker {online['taker']}; conversation created {created}"]
    if sent and online["taker"] and online["maker"]:
        first = sent[0][0]
        out.append(f"first request {first}, {sent[0][1]} B: {seconds(first) - seconds(online['taker']):.1f} s after the "
                   f"Taker came online, {seconds(first) - seconds(online['maker']):.1f} s after the Maker; relay peers then: "
                   f"Taker {relay_peers(taker, first)}, Maker {relay_peers(maker, first)}")
    out.append(f"Chat: the Taker sent {len(sent)} message(s), the Maker received {len(received)}")
    for stamp, digest, topic in publishes[:8]:
        out.append(f"publish {stamp} {topic} {digest[:14]}: {outcome.get(digest, 'no outcome line')}; Maker relay "
                   f"{at_relay.get(digest, 'never')}, handed to its Chat module {handed_on.get(digest, 'no')}")
    conversation = next((s for s in subscriptions[1:]), None)
    if conversation and sent:
        request = next((p for p in publishes if p[2] == conversation[1]
                        and seconds(p[0]) >= seconds(sent[0][0]) - 0.02), None)
        if request and request[1] in at_relay:
            out.append(f"the Maker subscribed to the conversation's topic at {conversation[0]}; the first request "
                       f"reached its relay at {at_relay[request[1]]}: margin "
                       f"{seconds(at_relay[request[1]]) - seconds(conversation[0]):+.3f} s")
    elif sent and not conversation:
        out.append("the Maker never subscribed to the conversation's topic in this phase")
    protocols = next((l for l in maker if "Configuration: Enabled protocols" in l), "")
    if protocols:
        found = re.search(r"(relay=.*)", protocols)
        out.append("Maker protocols: " + (found.group(1) if found else protocols)[:120])
    return out


if __name__ == "__main__":
    for path in map(pathlib.Path, sys.argv[1:]):
        print(f"== {path.name}")
        for line in summary(path):
            print(f"   {line}")
