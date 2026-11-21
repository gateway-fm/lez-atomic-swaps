# LEZ compute units: initialise, claim and refund (RFP Performance 01)

Measured against **LEZ v0.2.4** on the escrow program the deployment manifest
pins (program id `ca66e0dc…`). LEZ testnet 0.2 runs the program before the
custody fix of TM-0032 (`c22d61fc…`, deployed in block 10566); the fix added
165 to 185 cycles to every operation that reads the custody and 45 to
`AuthorizeNativeXmrClaim`.

## The unit

LEZ executes programs in a RISC Zero zkVM, so a program's compute cost is
**executor user cycles**, not an abstract gas unit. That is the unit LEZ's own
`tools/cycle_bench` reports for its built-in programs and feeds into its fee
model (`G_executor`), so the figures below are directly comparable to theirs.

Cycles are a deterministic function of the program and its inputs: these counts
reproduce exactly across runs, unlike wall-clock time.

## What was measured

| Operation | Instruction (tag) | User cycles | Segments | Share of the 32M limit |
|---|---|---:|---:|---:|
| Initialise escrow | `InitializeNativeWitnessed` (1) | 369,083 | 1 | 1.10% |
| Fund escrow | `FundNative` (2) | 379,093 | 1 | 1.13% |
| Claim | `ClaimNativeWitnessed` (4) | 442,097 | 1 | 1.32% |
| Refund | `RefundNative` (5) | 393,294 | 1 | 1.17% |

`MAX_NUM_CYCLES_PUBLIC_EXECUTION` in LEZ's state machine is 32 MiB of cycles
(33,554,432). Every escrow operation runs in a single segment at roughly one
percent of that ceiling.

Funding is listed because a BTC↔LEZ lock is two transactions — initialise then
fund — so locking costs the sum of the first two rows (748,176 cycles). Claim
and refund are the two mutually exclusive endings, each measured on the same
funded state.

## The ZEC path: tags 0, 2, 3 and 5

The ZEC↔LEZ pair locks LEZ under a SHA-256 hash/time lock on the same
program, with the same four instructions in both directions: the role selling
LEZ initialises (`InitializeNative`) and funds (`FundNative`), the LEZ buyer
claims by revealing the preimage (`ClaimNative`), and `RefundNative` is the
other ending. The sidecar builds exactly these in `prepare_native_escrow`,
`prepare_revealing_claim` and `prepare_native_refund`. Measured by a second test
in `escrow_cycles.rs`, with the same executor, the same embedded ELF and the
same pinned-program-id assertion, on rustc 1.96.0, Darwin arm64 (2026-11-21,
in the same run as the BTC and XMR rows):

| Operation | Instruction (tag) | User cycles | Segments | Share of the 32M limit |
|---|---|---:|---:|---:|
| Initialise escrow | `InitializeNative` (0) | 316,068 | 1 | 0.94% |
| Fund escrow | `FundNative` (2), on the preimage escrow | 364,089 | 1 | 1.09% |
| Claim | `ClaimNative` (3) | 397,911 | 1 | 1.19% |
| Refund | `RefundNative` (5), on the preimage escrow | 378,290 | 1 | 1.13% |

Locking costs 680,157 cycles. `FundNative` and `RefundNative` are the
instructions the BTC table lists too, measured again here because their cost
depends on the metadata they rewrite (see below).

| Operation | Accounts | Signer |
|---|---:|---|
| `InitializeNative` | 4 | depositor |
| `FundNative` | 3 | depositor |
| `ClaimNative` | 3 | claimant, revealing the preimage |
| `RefundNative` | 3 | none — permissionless at the boundary |

The inputs are the ones the escrow's guest test `recursive_atomicity.rs` and the
sidecar's planners build. Each measured instruction is also applied to the state
with `transition_from_public_transaction` at a timestamp inside its window, so an
input the program would reject fails the test instead of being measured.

## Why these instruction variants

One escrow program serves every pair, and its `ClaimAuthority` has three arms:
a SHA-256 preimage (the original ZEC path), a two-party aggregate witness (the
BTC path), and the XMR dual-adaptor path. The BTC↔LEZ swap uses the **witnessed**
variants throughout: the escrow's claim authority is the MuSig2 aggregate
account both roles jointly control, so the claim is authorised by an aggregate
signature rather than by revealing a preimage. The recorded run confirms it —
its escrow terms carry `aggregate_authority_account_id` and
`aggregate_x_only_public_key` and no secret digest — and the sidecar's asset
observation matches exactly this sequence: `InitializeNativeWitnessed` →
`FundNative` → `ClaimNativeWitnessed` → `RefundNative`.

The preimage variants (`InitializeNative` tag 0, `ClaimNative` tag 3) cost less
because the metadata they carry is smaller, but they are not what this pair
submits. `FundNative` and `RefundNative` are shared between both paths yet still
cost more here, because their cost depends on the state they run against: the
witnessed escrow's metadata holds an aggregate key and an authority account id
where the preimage escrow holds a 32-byte digest.

## The XMR path: tags 13–17

The Monero pair (TakerSellsLez only) runs on the same program, with
its own instructions for every step except funding, which is the shared
`FundNative`. Measured by the second test in `escrow_cycles.rs`, with the same
executor, the same embedded ELF and the same pinned-program-id assertion:

| Operation | Instruction (tag) | User cycles | Segments | Share of the 32M limit |
|---|---|---:|---:|---:|
| Initialise escrow | `InitializeNativeXmr` (13) | 515,523 | 1 | 1.54% |
| Fund escrow | `FundNative` (2), on the XMR escrow | 473,202 | 1 | 1.41% |
| Publish the claim partial | `AuthorizeNativeXmrClaim` (14) | 414,196 | 1 | 1.23% |
| Claim | `ClaimNativeXmr` (15) | 536,531 | 1 | 1.60% |
| Refund | `RefundNativeXmr` (16) | 537,016 | 1 | 1.60% |
| Punish | `PunishNativeXmr` (17) | 488,586 | 1 | 1.46% |

`FundNative` is the same instruction as in the BTC table but costs more here
(473,202 against 379,093 cycles), for the reason given above: it reads and
rewrites the escrow's metadata, and the XMR metadata (version 3) carries two
aggregate keys and accounts, two DLEQ transcript commitments, the claim-partial
binding and commitment and the punish time.

Locking LEZ is initialise then fund (988,725 cycles). The happy path adds the
Taker's `AuthorizeNativeXmrClaim` and the Maker's `ClaimNativeXmr`; the other
endings are `RefundNativeXmr` (the Taker, from the refund time) and
`PunishNativeXmr` (the Maker, from the punish time). Claim runs on the
authorised state; refund and punish each run on the funded state, as the swap
can submit them. The inputs are the ones the escrow's guest tests
(`recursive_witnessed_claim.rs`) and the sidecar's native-XMR planners build:
account orders, signers, a claim-partial commitment that matches the published
partial, and distinct claim and refund MuSig2 aggregates whose two-party
signatures authorise tags 15 and 16. Every input is a local state the test
builds; none needs a chain.

The program is named `zec_escrow_v02` for historical reasons — ZEC was the first
pair implemented against it, and the BTC and XMR paths were added to the same
program rather than to new ones.

## The program these numbers describe

| | |
|---|---|
| Program | `zec_escrow_v02`, the one escrow program of every pair |
| ImageID / program id | `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98` |
| Guest ELF SHA-256 | `3a016fd0059390384da2d6f86d366e6f2036b6ffdda3c8b5cd4a7d965bdc47f3` (488,748 bytes, stripped) |
| LEZ | v0.2.4, commit `47eba256479f6f785acbd138834340703cd03401` |
| spel | commit `7f13e71f91372a32e26b71d19dbbb60532711048` |
| RISC Zero | 3.0.5 (`r0vm` 3.0.5, guest builder `r0.1.94.1@sha256:c2f63fdd…`) |
| Measured on | rustc 1.96.0, Darwin arm64, 2026-11-21, every row in one run |

The benchmark runs against the **pinned ELF**, not a rebuild:
`methods/build.rs` takes the binary through `LEZ_V02_PREBUILT_GUEST_ELF`, runs
`r0vm --elf … --id`, and refuses to build unless the ImageID matches the one
pinned in `guest/deployment-manifest.toml`. The test then asserts the embedded
`ZEC_ESCROW_V02_ID` equals the pinned program id, so a drifting binary fails
rather than silently reporting the wrong program's cost.

The deployed release is identified by the program set the endpoint publishes,
since the sequencer exposes no version method:

```
authenticated_transfer     fe96c4228babbe8bc578e3e25b884cacb07f8c86541f27ed676789875eef875a
token                      ccc4713e2b5ecdff37b0c67c295369effc04b7e8994eb11c3f410bb226b82e9b
amm                        4f034069146665de979ef67a8bffa45a8d615612757e38fef16aea1675de6a2f
pinata                     fc52f17a60f8b5e8de28e1a8c3133c012485011a36aef985ce24d69ff4f3528c
privacy_preserving_circuit 383e884f67e016e9e046294a6f8ed2dab5b516bbcf452c18f32145f2d400206f
```

## How to reproduce

```sh
export LEZ_V02_PREBUILT_GUEST_ELF=/path/to/zec_escrow_v02.bin   # the pinned, stripped guest
cd compat/lez-v0.2-provisional/escrow/methods
cargo test --test escrow_cycles -- --nocapture
```

The test builds each operation's public transaction exactly as the swap does,
then runs the guest the way the chain does. LEZ executes a program through
`Program::execute`, which is crate-private, so the harness mirrors it: the same
four inputs in the same order — program id, caller program id (`None`, since the
swap calls the escrow directly), pre-states, instruction words — through
`default_executor().execute(env, elf)`, with no proving.

`is_authorized` on each pre-state means "this account signed the transaction",
matching how the state machine fills pre-states:

| Operation | Accounts | Signer |
|---|---:|---|
| `InitializeNativeWitnessed` | 5 | depositor |
| `FundNative` | 3 | depositor |
| `ClaimNativeWitnessed` | 4 | the aggregate authority |
| `RefundNative` | 3 | none — permissionless at the boundary |
| `InitializeNativeXmr` | 6 | depositor (the Taker) |
| `FundNative` (XMR escrow) | 3 | depositor |
| `AuthorizeNativeXmrClaim` | 2 | depositor |
| `ClaimNativeXmr` | 4 | the claim aggregate authority |
| `RefundNativeXmr` | 4 | the refund aggregate authority |
| `PunishNativeXmr` | 3 | claimant (the Maker) |

The claim's aggregate signature is produced by a real two-party MuSig2 signing
session over the claim message, as Maker and Taker do in a live swap; so are
the XMR claim's and refund's, each under its own aggregate. Each measured
instruction is then applied to the state with `transition_from_public_transaction`
at a timestamp inside its window, so an input the program would reject fails
the test instead of being measured.

## Why these are not read back from the testnet

The public sequencer does not report per-transaction compute. `getTransaction`
returns only the transaction bytes and its block id, and there is no receipt,
version, or `rpc.discover` method. Cycle cost is therefore measured
executor-side against the pinned binary, which is also what makes it a
reproducible benchmark rather than an observation of one run.
