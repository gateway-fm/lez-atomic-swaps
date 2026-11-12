# Provenance — full regression of `fix/103-review-items`, 2026-11-12

Everything in this folder was produced on one host by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh` (the desks driven in each party's own Basecamp through
`ui-tests/verify.mjs`), three at a time on three regression lanes.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines and captions;
the two Maker Node logs in `live-checks/` carry actor effect lines and
error classes. No key, cookie, capability file, `runtime/` or `market/`
content is included. Local paths are replaced by `<checkout>`, `<home>` and
`<scratch>`. Scanned before commit with the repository's gitleaks
configuration.

## Source

The branch is `fix/103-review-items`, on `main` `af3ac978` (PR #120 and PR
#124 squashed). `main`'s squash dropped the commit ids older documents quote
(`023c929e` and the like); a tree hash names the same content whatever the
history, so this set gives both.

| what | commit | tree of `crates` | `compat` | `apps` | `modules` | `deploy` |
|---|---|---|---|---|---|---|
| images built at | `964cb8c8` | `f41c6a2c` | `f35496e4` | `35798988` | `b3e1af6e` | `8dc5cac0` |
| the run's tests | `2b6569e0` | same | same | same | same | `f64179e2` |
| the rerun's tests | `62cf95ca` | same | same | same | same | `ad91fe94` |

(`git rev-parse <commit>:<directory>`; full values:
`f41c6a2c69aa1a3dbcf5465b27d4c8a43ae46d5b`,
`f35496e40cd49171dc0e6cd964a6d0072c29dc22`,
`35798988918fda8339d196b09010ab37b2d75ed3`,
`b3e1af6e769a383edd56995d43db7adb39085213`,
`8dc5cac07d24ceb7667a051d17129adba9260fa0`,
`f64179e2e49003c141b4d653d54783ab695b72c4`,
`ad91fe94ab1f1d2ee2849bc80e6e6198a6b9ffa6`.) `regress.log` prints the
commit and "tree dirty: 0" at its start. `2b6569e0` differs from `964cb8c8`
in `deploy/scripts/node-e2e.py` alone, `62cf95ca` from `2b6569e0` in
`deploy/ui-tests/verify.mjs` alone: test code, read from the checkout, in no
image.

What the branch changes over `main`, in order: the item-by-item notes for
#103; the harness for items 8, 9, 10, 12 and 14 (`3f275a49` to `fcf9860e`);
`last_outcome` and `waiting_for` in both Nodes' swap views, the desk rows and
the header badge for items 6 and 7 (`16974fd2`, `33f0f939`, `a184050d`,
`36cfd863`); then what the first full run found: the Maker supervisor
(`9579950b`), the Bitcoin actor's late LEZ claim lookup (`48c8b5b6`,
`964cb8c8`), the sidecar's discovery by terms (`17cfa50a`), each after its
failing test, and four harness fixes (`d685ecc2`, `d3131654`, `2b6569e0`,
`62cf95ca`).

## Images

| image | id | built (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:b2e98599d5510cc535df23cf762a8cf002d526e3c8d12c50d18a9986f2169530` | 2026-11-12 03:02 |
| `lez-taker-node:local` | `sha256:9b1abd37a4fabb20572006dc92891b75e0bba34effb45f077e68abf30bf63939` | 2026-11-12 03:02 |
| `lez-basecamp-ui:local` | `sha256:c3134f665d1aafb17358f7e75ee2e3a6acf976e48e144879560ea44d0141ab22` | 2026-11-11 21:35 |
| `lez-network-module:local` | `sha256:51a018ec2bceeacb4123b8607d4b2f280377b52729a0f254d251ccbeaf668449` | 2026-11-11 21:35 |
| `lez-services:local` (sequencer) | `sha256:ce3eb7aeaa5ab280a0be3bcba875e502d349f0c1b57b8788101e937e4d3fa3a8` | 2026-11-11 15:01 |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:8093a13ad9cfd754315da4d5b152865c6e35c6dea868abb7b753fa55f5c0180c` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:a2fc0c4660aba5b8a2dfd044c1d5f751d17f36b82a79912a4acf253d699ca4cd` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:6d20393814e0e4c5244df85f21279fe024f54c10b4440e1c618c037c7ee61fe6` | 2026-11-11 15:01 |

Read at the start of the run (`docker image inspect`) and not rebuilt during
it or the rerun. Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`:
`964cb8c852f4b78d5472b4639b59c77b80ab6acf`; the role Node package inside
names the same commit for the Node and `17cfa50a` for the sidecar (the last
commit that changed it). The desk packages and the network module were built
by the landing of the desk changes (2026-11-11 21:35Z); `apps`
and `modules` did not change after it. These are local ids of images built
on this host (linux/amd64); no image was pulled from a registry and no
release was built from this branch.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Nothing else of this
  project ran on the host during the run; other projects' containers share it.
- `up.sh --fresh-lez` (LEZ_LANES=3) at 03:29Z made the chain the run started
  on; `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` ran from
  03:32:11Z to 05:49:59Z (137 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each
  with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ network.
  `results.tsv` names the lane of every test.
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the last
  two tests). No chain was replaced for a finality freeze: 0 frozen tries.
  Every try's start and verdict line in `logs/regress.log` carries the
  finalized height and the tip.
- The rerun (05:50Z to 06:00Z, three tests, three lanes) ran on the second
  balance-refusal chain, same images.

## Earlier runs of the branch (`earlier-runs/`)

The runner's log and table of each, and the logs of the tests named in
`RESULTS.txt`:

| run | source | what |
|---|---|---|
| `20261111T223238Z` | `36cfd863`, images of `36cfd863` | first full run: 93 of 97 in 165 min |
| `20261112T012540Z` | `d3131654`, Node images rebuilt | reruns: 3 passed; `late-claim-unmined` selling LEZ hung, stopped by hand before its verdict |
| `20261112T021134Z` | `17cfa50a`, sidecar and Node images rebuilt | reruns: 3 passed; the same test hung again, stopped by hand |
| `20261112T030607Z` | `964cb8c8`, Node images rebuilt (this run's images) | reruns: 2 passed, 1 failed on a harness race |
| `20261112T031043Z` | `964cb8c8` | the failed one again: passed |
| `20261112T032252Z` | `964cb8c8` | the landing's smoke: 6 of 6 |

`live-checks/` (2026-11-11, before the first run; its `README.txt`) holds the
hand checks of #103 items 6 and 7 on images of `caebd6d9` and `36cfd863`, and
under `late-claim-unmined-sells-lez-fail/` and `-followup/` what was captured
from the two hung tries: the Maker Node's log, and for the second both Nodes'
swap views.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
Seven are in `videos/`, the ZEC demo set: `zec-happy`, `zec-maker-refund` and
`zec-concurrent` in both directions and `zec-stale-window` selling ZEC. Five
are from the run; the two `zec-concurrent` ones are from the rerun at
`62cf95ca` (selling ZEC failed in the run at its desk step; selling LEZ passed
in both and the later recording was taken). They replace the seven of
`regression-m2-2026-11-11/videos/`, removed in the same commit. The other
recordings (31 from the run, 3 from the rerun, these seven among them) are
kept outside git; `videos-kept-outside-git.sha256` lists them.

## Not covered by this set

- Any public network: N/A for this release. The public LEZ testnet is 0.3,
  incompatible with this v0.2.4 stack; postponed to the 0.4 port (#107).
- linux/arm64 and macOS: every image here is linux/amd64.
- The release workflow and a release bundle: none was built from this branch.
- A full run at `62cf95ca`: the last commit changes one desk-test step and was
  run for three tests.
- The token-asset claim lookup, a claim more than 4,096 LEZ blocks after the
  swap's start, and a Maker LEZ claim that misses the Taker's refund time
  (`docs/issue-closing-notes-2026-11-11.md`, item 14): no test.
