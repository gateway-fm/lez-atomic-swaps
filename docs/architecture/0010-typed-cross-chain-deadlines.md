# ADR 0010: Typed consensus clocks and cross-chain safety bounds

Status: Accepted; coordinator integration and parameter calibration pending —
2026-07-11

## Context

The prototype compared two normalized `u64` values even though LEZ may use block
height or timestamp while BTC, XMR, and ZEC use their own consensus clocks. A
Bitcoin height and LEZ timestamp have no meaningful numeric ordering. RFP R6
also requires variance, congestion, reorgs, and drift to be explicit.

## Decision

Every refund point is a `ChainPosition { chain, basis, value }`, where basis is
block height or consensus timestamp. Deadline checks reject a different chain or
basis instead of comparing raw values. `RefundSchedule` maps maker/taker roles to
LEZ/foreign chains using immutable `SwapDirection`.

Cross-chain ordering is validated separately with conservative Unix-time bounds:

    taker_refund_earliest >= maker_refund_latest + required_reaction_margin

`maker_refund_latest` includes the slowest plausible maker-chain clock advance,
congestion, inclusion, configured reorg recovery, and sequencer admission-to-
validation slack. `taker_refund_earliest` uses the fastest plausible taker-chain
clock including permitted clock drift. These values validate negotiated terms;
runtime expiry still uses only the relevant chain's typed consensus position.

## Consequences

Pair adapters own conversion from current tips/timestamps and reviewed network
parameters into deadlines and conservative bounds. The coordinator never invents
block-time conversions. Parameters name their network/release and are covered by
boundary tests; unsafe arithmetic, zero margin, wrong role chain, or wrong clock
domain is rejected.

The standalone typed schedule and three acceptance tests are implemented. The
legacy coordinator `Timelocks` and CLI inputs must be migrated before this M1
gate is complete; no later milestone may add chain adapters to the normalized
API.
