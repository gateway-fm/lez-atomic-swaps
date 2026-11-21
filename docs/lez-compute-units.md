# LEZ compute units: initialise, claim and refund (RFP Performance 01)

Measured against **LEZ v0.2.4** on the escrow program the deployment manifest
pins (program id `ca66e0dc…`). LEZ testnet 0.2 runs the program before the
custody fix of TM-0032 (`c22d61fc…`, deployed in block 10566); the fix added
165 to 185 cycles to every operation that reads the custody.

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
| Measured on | rustc 1.96.0, Darwin arm64, 2026-11-21 |

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

The claim's aggregate signature is produced by a real two-party MuSig2 signing
session over the claim message, as Maker and Taker do in a live swap.

## Why these are not read back from the testnet

The public sequencer does not report per-transaction compute. `getTransaction`
returns only the transaction bytes and its block id, and there is no receipt,
version, or `rpc.discover` method. Cycle cost is therefore measured
executor-side against the pinned binary, which is also what makes it a
reproducible benchmark rather than an observation of one run.
