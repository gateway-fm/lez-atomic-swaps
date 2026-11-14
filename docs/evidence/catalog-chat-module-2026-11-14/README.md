# One install of a desk plans one Chat and one Delivery — 2026-11-14

Issue #147: with Logos's `chat_module` 0.2.2 in the graph, one
`package install` of a 0.3.1 desk planned `delivery_module` twice (0.2.1 for
the desk's `~0.2.0`, 0.3.3 for Chat's bare name), and Basecamp 0.3.2 refused
the desk when 0.3.3 was written last. This folder records the fix: the
catalog's `chat_module` 0.2.2-delivery.1 (`deploy/scripts/package-chat-module.sh`:
the release's files, `delivery_module ~0.2.0`), which the desks require
exactly. How it was run, and what was not: [PROVENANCE.md](PROVENANCE.md).

## The packages

`packages.tsv` is the set as `pin-package-dependencies.sh` wrote it: the
merged v0.3.1 packages, the official `delivery_module` 0.2.0 and
`blockchain_module` 0.2.4 (for their versions), and the cut Chat.

| package | version | dependencies |
|---|---|---|
| lez_atomic_swap_maker | 0.3.1 | chat_module 0.2.2-delivery.1; delivery_module ~0.2.0; lez_core 0.4.1-sign.4; lez_swap_node 0.3.1; lez_maker_node 0.3.1 |
| lez_atomic_swap_taker | 0.3.1 | chat_module 0.2.2-delivery.1; delivery_module ~0.2.0; lez_core 0.4.1-sign.4; lez_swap_node 0.3.1; lez_taker_node 0.3.1 |
| chat_module | 0.2.2-delivery.1 | delivery_module ~0.2.0 |

Before, the desks declared `chat_module ~0.2.2` and Logos's Chat 0.2.2 a bare
`delivery_module`.

`chat-module/build.log`: the script fetched the release asset, checked its
sha256, and wrote the package; every member but `manifest.json` is the
release's byte for byte, the manifest differs in `version` and
`dependencies` only, the root hash is the release's
(`9f626b21…3366`), `lgx verify` passes, and it carries `darwin-arm64`,
`linux-amd64` and `linux-arm64`. The same input gives the same bytes
(sha256 `b2b0f128…df29e`, built twice, to two output names).

## One install plans each package once

`install/macos-maker.log`, `install/macos-taker.log` (logosctl 0.3.2, macOS
arm64) and `install/linux-arm64-*.log` (logosctl 0.3.2 in a Debian trixie
arm64 container), each a fresh session with the local catalog beside the
official one:

```
plan: delivery_module 0.2.1 (official); chat_module 0.2.2-delivery.1 (local); lez_core 0.4.1-sign.4 (local);
      lez_swap_node 0.3.1 (local); lez_maker_node 0.3.1 (local); lez_atomic_swap_maker 0.3.1 (local)
deps: chat_module 0.2.2-delivery.1 installed; delivery_module ~0.2.0 -> 0.2.1 installed;
      lez_core, lez_swap_node, lez_maker_node installed
```

The Taker's is the same with `lez_taker_node`. No `version_mismatch`, one
`chat_module`, one `delivery_module`, and none of the RLN or libp2p modules
Delivery 0.3.3 brought in. `install/macos-both.log` installs the Maker, then
the Taker, into one session; the second install plans only the Taker's two
packages.

## The desk opens after one install

| | macOS arm64 (Basecamp 0.3.2) | Linux arm64 (Basecamp 0.3.2, Xvfb) |
|---|---|---|
| `logosctl package install`, then Basecamp on its packages | `basecamp/macos/l1-maker-excerpt.log`, `l1-taker-excerpt.log` | `basecamp/linux/l1-*-excerpt.log`, `L1-2-maker-open.png`, `L1-3-taker-open.png` |
| Applications → tile → **Install** → **Launch** | `basecamp/macos/u-*-excerpt.log`, `ax-*` | `basecamp/linux/u2-maker-excerpt.log`, `u3-taker-excerpt.log`, `U2-*.png`, `U3-*.png` |

Each excerpt shows `Loading core dependencies for "lez_atomic_swap_<role>"`
and `ui-host: loaded plugin "lez_atomic_swap_<role>"`, and none `blocked by
deps`. The Applications page's dialog lists six required packages (the desk,
Delivery `v.0.2.1`, Chat `v.0.2.2-delivery.1`, `lez_core`, `lez_swap_node`,
the role's Node package) and no optional ones; after the one Install click
all six read INSTALLED and the desk launches (`U2-6`, `U2-7`; on macOS the
accessibility dumps `ax-*-installed.txt`, `ax-*-launched.txt`, whose text
includes "LEZ — Maker Desk" / "LEZ — Taker Desk"). The desks show "Node
unavailable": these Basecamps were started without the Nodes' environment.

## Chat reads Delivery 0.2.1

Each excerpt ends with the count of Delivery `message_received` events and of
Chat's `messageReceived payload missing or malformed` warnings. Chat joined
Delivery on `logos.test` ("delivery is online"); Delivery received up to 896
messages in a session (`basecamp/linux/u3-taker-excerpt.log`), and Chat's
decode failures are 0 in every session. The run with Delivery 0.3.3 in
place of 0.2.1 logged 10,831 of them.

## Swaps with the fixed install

`swaps/`: both desks in one native Basecamp 0.3.2 on macOS, on the user
directory that received them from one `package install` each
(`install/macos-both.log`: `chat_module` 0.2.2-delivery.1, `delivery_module`
0.2.1). Opening LEZ Maker, then LEZ Taker, started both signers and both
Nodes from the installed role packages; `node-e2e.py` then ran against the
native Nodes on the devnet's chains (a fresh LEZ chain, fast timing profile).

| scenario | direction | swap | result |
|---|---|---|---|
| BTC `happy` | TakerSellsForeign | `1c5dcc0be066` (Bitcoin claim `11e083094c10`, Taproot key path) | passed, 41 s |
| BTC `happy` | TakerSellsLez | `9aa57ce70ece` (Bitcoin claim `148dc5296e1c`, Taproot key path) | passed, 32 s |
| ZEC `zec-happy` | TakerSellsLez | `zec-devnet-swap-1794675248` (Maker funding `9c850875…` in Zebra block 717, Taker claim `c82604fc…` in block 719) | passed, 72 s |
| ZEC `zec-happy` | TakerSellsForeign | `zec-devnet-swap-1794675320` (Taker funding `d8ad6eba…` in block 720, Maker claim `4da2f146…` in block 722) | passed, 62 s |

Pass and fail are read from each scenario's last line (`=== <scenario>:
passed`), not from `results.txt`. In that session (`swaps/basecamp-excerpt.log`)
Delivery received 1,385 messages and Chat logged 0 decode failures; no desk
was `blocked by deps`.

## Pre-releases and ranges

`probe/`: a package published only as `0.2.2-delivery.1`, required four
ways, in fresh logosctl 0.3.2 sessions. The exact version resolves; `~0.2.0`
and `~0.2.2` fail with `RESOLVE_FAILED ... no candidate matches`; a bare name
takes it only because nothing else carries that name. So the official
packages (`chat_ui` 0.3.0 requires `chat_module >=0.3.0`; older ones a bare
name, which takes the newest, 0.3.0) never resolve to the catalog's Chat,
and a desk's exact requirement resolves to nothing else.

## Logos's own Chat app beside a desk

`install/chat-ui-coexist.log`. Installing `chat_ui` (0.3.0) after a desk
upgrades `chat_module` to 0.3.0 and `delivery_module` to 0.3.3; the desk's
`package deps` then reports both `version_mismatch`. Installing the desk
after `chat_ui` does the reverse, and `chat_ui` reports the mismatches. The
installer does not weigh installed packages' requirements; one name has one
directory.

## Download source

`install/macos-maker-default-source.log`: the same install with the default
source (`any`) took 34 s, as with `http` (34 s); its daemon log records no
storage download. The half-minute storage attempt the install guide warns of
was seen in an earlier Basecamp 0.3.2 session (a `StorageFetcher` request
for a package Delivery 0.3.3 pulled in, 35 s before it failed and the
download went over HTTP), not in this run.

## The Compose images

`image-set.txt`: the images' set (Chat 0.2.2 built from its flake) pinned by
the script before and after this change gives the same files byte for byte,
`packages.tsv` included. The images do not change.
