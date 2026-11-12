# Untrusted-input fuzz runs, 2026-11-12

Twenty minutes each of three cargo-fuzz targets over the bytes a stranger hands a Node before any
swap exists. No crash, timeout or out-of-memory in any of them.

## What ran
- Source: commit `8b79df19b66e1e1da610090ad17bde92028c6d3c`; trees of `fuzz`
  `df5b88647ed0863321d552556160eafaba525148`, `crates/node-common`
  `2cf5fa3e2351d0832b39b7c25d9775381266d292`, `crates/swap-store`
  `82cc6041488630f691f26f08f28075a3cf76f6de`, `crates/zec-swap-sdk`
  `a70575cd0ed6ad66b5ade15666ca10b10904e8e7`.
- Container: `rust:1.96.0-bookworm`, image `sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc`,
  repository mounted read-only. Toolchain: `nightly-2026-06-01` (rustc 1.98.0-nightly 14210df0e
  2026-05-31, LLVM 22.1.6), `cargo-fuzz 0.13.2`, `libfuzzer-sys =0.4.13`; release with debug
  assertions, AddressSanitizer. The coordinator run used `nightly-2026-07-01`, which cannot
  compile pasta_curves 0.6.1 (a new built-in `unroll` attribute); that crate is in the Zcash SDK's
  graph, which node-common and swap-store pull in.
- Command: `scripts/run-fuzz.sh <target> 1200` (FUZZ_JOBS=6), one target after the other on the
  shared 8-core host: `cargo fuzz run` with `-fork=6 -max_total_time=1200 -timeout=2` from the
  checked-in seeds, then `cargo fuzz cmin`. `-max_len` is one byte over the decoder's own bound
  (announcement 32769, agreement record 16385); Chat frames are bounded at 1 MiB, the run at 16384.

## Results (`fuzz-summary.txt` holds the libFuzzer lines)
| Target | Executions | exec/s (6 jobs) | Edges (start → end) | Corpus seeds → grown → cmin | Crash / timeout / OOM |
|---|---|---|---|---|---|
| `offer_announcement` | 28,688,380 in 1218 s | 23,550 | 2034 → 4401, still rising | 8 → 3392 → 2418 | 0 / 0 / 0 |
| `chat_frame` | 56,495,193 in 1221 s | 46,270 | 1677 → 3707, still rising | 5 → 2337 → 1739 | 0 / 0 / 0 |
| `zec_agreement_wire` | 66,113,481 in 1210 s | 54,640 | 1027 → 1461, flat from ~300 s | 9 (5 kept by the first merge) → 205 → 190 | 0 / 0 / 0 |

The grown corpora are over the 50-file bound for checking in; `fuzz/corpus/<target>` holds only the
seeds, which `cargo test -p lez-node-common --test fuzz_corpus_replay` and
`cargo test -p lez-zec-swap-sdk --test fuzz_corpus_replay` replay on stable.

## What each target reaches
- `offer_announcement`: an announcement as `LogosChatGateway::ingest_offer_announcement` takes it on
  a Taker (canonical Base64, the 32 KiB bound, JSON, canonical re-encoding, lease times, outer
  signature, nested offer envelope, offer index), and the same bytes as bare `MakerOfferV1` JSON
  through `validate()` and `quote_foreign_amount` at and around its bounds. Oracles: a rejection
  leaves the index as it was; an accepted announcement is a replay the second time and verifies on
  its own to the same bytes; a validated offer round-trips through JSON and quotes without panic.
- `chat_frame`: Chat message text from the bound peer through `LogosChatGateway::ingest` on a Maker
  (requests) and a Taker (responses): the 1 MiB bound, JSON, role, method allow-list and the
  content-addressed frame id. Oracles: a rejection changes no counter and queues no reply; an
  accepted frame is a replay the second time.
- `zec_agreement_wire`: the bytes through `ZecAgreementDraftV1::from_wire_at` (the Maker's
  `zec_chat_propose_v1`), `ZecMakerAgreementProposalV1::from_wire_at` (the Taker) and
  `ZecAgreementV1::from_wire_at` (the Maker's `zec_chat_complete_v1`). Oracle: whatever decodes
  re-encodes and decodes to the same value.

## What this does not cover
- Signatures gate depth. A mutated announcement fails its signature, so the nested offer is reached
  only from the bare-offer path; a mutated proposal or agreement fails its Maker or Taker signature,
  so only the unsigned draft's validation is explored past decoding (hence the early plateau); a
  mutated Chat frame fails its frame id, so past that point only the seeds' frames are accepted.
- An accepted Maker request is forwarded unparsed to the Maker's own Chat socket; the per-method
  JSON parameter decoding there (jsonrpsee, `chat_contracts.rs`) and the Bitcoin Chat wire decoders
  (`BtcRoleContributionV1`, `BtcAgreementDraftV1`, `BtcAgreementV1::from_wire`) are not fuzzed.
- No chain or sidecar input: the Zcash transaction parse in `spend_observation.rs` and Bitcoin
  adaptor-secret extraction are not fuzzed. Twenty minutes per target is a smoke run, not a campaign.
