# Remote signing for the BTC and LEZ legs

Status: research plan, 2026-09-09, for ADR 0213 §8 / S3.1–S3.2; BTC↔LEZ only.
It replaces the Bitcoin and LEZ sections of the root draft
`WALLET_REMOTE_SIGNING_RESEARCH.md`; claims were checked against the working
tree (2026-09-09); corrections are marked ✎.

## 1. Where keys are used today

| # | Leg / role | Code | Key | What is signed | Interactive / time-bound |
|---|---|---|---|---|---|
| B1 | BTC + LEZ claim, both roles | `crates/adaptor-signature/src/lib.rs:655` `sign_persisted_adaptor_partial`, driven by `crates/adaptor-role-runner/src/ceremony.rs:176` `CeremonySeat::accept_nonce_sign` | `agreement.key` (`crates/btc-role-preflight/src/lib.rs:794` `RoleSecret::Agreement`). Maker reuses its long-lived `btc-chat-signing.key` (`crates/maker-node/src/btc_lifecycle.rs:349` `agreement_key`); Taker mints one per swap (`taker_service/btc_dynamic.rs:377`) | MuSig2 adaptor partial (`musig2::adaptor`) over (a) the BIP-341 key-path `SIGHASH_DEFAULT` digest of the cooperative claim (`crates/btc-swap-sdk/src/transaction.rs:128` `CooperativeKeyPathSpend`, key tweaked by the CSV-tapleaf root) and (b) the untweaked LEZ `Message::hash()` (`agreement_v1.rs:1905` `adaptor_session_context`) | Yes: three Taker-driven Chat rounds `btc_ceremony_reserve/nonce/partial_v1` (Maker handlers `btc_lifecycle.rs:764/880/926`). No protocol deadline; bounded by the 25 s Chat wait (`node-common/src/logos_chat_gateway.rs:49`), 30 s owner RPC (`local_rpc.rs:19`) and offer TTL |
| B2 | BTC refund, funder | `crates/btc-reference-actor/src/lib.rs:4869` `prepare_bitcoin_refund_effect` | `bitcoin-refund.key`, copied into the actor bundle (`btc-role-lifecycle/src/actor.rs:73-90`) | BIP-340 `sign_schnorr_no_aux_rand` over the BIP-342 script-path digest of the CSV tapleaf (`transaction.rs:308` `RefundScriptPathSpend`) | No; CSV window (144 blocks locally) |
| B3 | Claims, claimant | `btc-reference-actor/src/lib.rs:4933` `prepare_bitcoin_claim_effect`, `:5076` `lez_claim_aggregate_signature` → `adaptor-signature:820` `adapt_presignature`, `:853` `extract_adaptor_secret` | Taker-only `adaptor-scalar.key` (t); the Maker extracts t from the on-chain signature and checks tG = T | Completes the stored 65-byte presignature | No; claim window |
| B4 | BTC funding, funder | `btc-role-lifecycle/src/funding.rs:204` `walletcreatefundedpsbt` → `walletprocesspsbt` → `finalizepsbt` | Bitcoin Core wallet `LEZ_BTC_WALLET` | Funding inputs; ✎ the funding key never enters Rust | No; pre-lock |
| B5 | Agreement, both | Maker `maker-node/src/btc_chat.rs:391`; Taker `btc_dynamic.rs:692`; contribution `btc-role-preflight/src/lib.rs:471` | same `agreement.key` | BIP-340 over the 32-byte agreement/contribution commitment (TM-0004) | One Chat round-trip; offer TTL |
| B6 | Delivery, Maker | `node-common/src/run_local_delivery.rs:760` `signed_offer_envelope`, `:367` `sign_logos_offer_announcement` | `delivery-signing.key`, plaintext, loaded once (`bin/lez-maker-node.rs:1282`) | ✎ secp256k1 **ECDSA** (low-S) over domain-tagged digests, not Schnorr | Announcement re-signed every 10 s (TTL 30 s) |
| L1 | LEZ escrow, depositor; revealing claim, claimant | `compat/lez-v0_2-sidecar/src/native_prepare.rs:5077` `prepare_message` ← `plan_witnessed_pair` (InitializeNativeWitnessed + FundNative), `plan_pair`, `prepare_revealing_claim` | `lez-signer.key`, provisioned by the example binary `lez-v02-local-actor-identity` (`actor_identity.rs:84`), passed as `--private-key-file` on the sidecar argv (`btc-role-lifecycle/src/sidecar.rs:166`) | k256 BIP-340 over the borsh `Message::hash()` via upstream `WitnessSet::for_message`, OsRng aux | No, but nonce-pair reservation is order-sensitive; bounded by the escrow `refund_at` |
| L2 | LEZ witnessed claim | `native_prepare.rs:1973` `complete_witnessed_claim` | none: verifies the aggregate signature from B3 under the untweaked MuSig2 key and installs it | — | — |
| L3 | LEZ refund | `native_prepare.rs:1766` `prepare_native_refund` | none: unsigned `WitnessSet` (permissionless ABI) | — | — |

Nonce custody: secret nonces are plaintext in the per-leg SQLite journal until
the partial is persisted (`crates/swap-store/src/adaptor_session_journal.rs:7`,
`:749` `sign_and_persist_partial`); a UNIQUE fingerprint blocks cross-session
reuse. No signer trait exists for BTC or LEZ; ZEC has one
(`crates/zebra-node-adapter/src/claim.rs:36` `ZebraClaimSigner`).

## 2. What a remote signer must support

- **Adaptor partial (B1).** Inputs: ordered keys `[maker, taker]`, merkle root
  (BTC only), 32-byte message, adaptor point T, session id. Nonce =
  `SecNonce::generate(rand, key, aggkey, msg, session_id)` (`adaptor-signature:257`);
  commitment = SHA-256 over role‖session‖keys‖Q‖msg‖T‖pubnonce (`:935`);
  partial = `musig2::adaptor::sign_partial`. Three ordered outputs per leg
  (commitment, public nonce, partial), the nonce consumed exactly once and
  bound to `durable_context_binding` (`:186`); peer input is needed between
  outputs; both legs advance in lockstep.
- **Refund (B2), agreement and contribution (B5):** plain BIP-340 over a
  32-byte digest. The refund message is fixed before the lock (ADR 0044) and can be
  signed during the ceremony.
- **LEZ transactions (L1):** BIP-340 over a 32-byte prehash the signer must
  recompute from the exact borsh message (program, accounts, nonces,
  instruction), never sign blind.
- **Funding (B4):** PSBT, any wallet. **Delivery (B6):** ECDSA every 10 s,
  never push-button.

## 3. Candidate wallets

Bitcoin, BIP-327 partials: Ledger Bitcoin app ≥ 2.4.0 (`musig()` policies, two
`SIGN_PSBT` rounds, nonce state in flash between rounds; ledger.com/blog-musig2-ledger-bitcoin-app,
app-bitcoin-new `doc/musig.md`); Coldcard EDGE ≥ 6.5.0X, developer track only
(bitcoinops.org/en/newsletters/2026/04/17); Bitcoin Core 31.0 (PR #29675;
secnonces in memory, lost on restart); Nunchuk (software keys only); lnd
`signrpc` MuSig2 sessions (programmatic, tweaks only). HWI 3.2.0 carries the
BIP-373 fields; device signing is draft PR #794. No partials: Trezor (#1946
open), BitBox02, Jade, Keystone, Passport, Sparrow (BIP-322 only).

**Adaptor partials: none.** All of the above sit on upstream libsecp256k1's
musig module, whose `nonce_process` has no adaptor argument; adaptor MuSig2
exists only in secp256k1-zkp, the `musig2` crate this repository already uses,
and `schnorr_fun`. BIP-373 has no adaptor fields. Raw 32-byte BIP-340 signing:
Ledger/Trezor `signMessage` are BIP-137 ECDSA; lnd `SignMessage` always
hashes. ✎ The draft's Sats Connect/UniSat/Leather candidates are PSBT-only
browser providers and fit B4 only.

LEZ: accounts sign k256 BIP-340 over a 32-byte prehash (`lee/state_machine/src/signature/mod.rs`;
dev-branch `AccountManager::sign_message([u8;32])`), so the LEZ leg is
"sign 32 bytes". `logos-blockchain-key-management-system-service` is an
in-process Overwatch service with Ed25519/X25519/ZK keys only: no secp256k1, no
RPC. The v0.2.0 `wallet` crate stores plaintext JSON (LOGOS-018) with no signing
RPC; `wallet-ffi` has no generic sign; the Basecamp `logos-wallet-module` is
"very wip" without a sign API; `logos-accounts-module` was archived 2026-08-13. The only hardware path is Bitgamma's LEE Keycard applet
(`SCHNORR_BIP340`, custom cap file, public accounts only). **No BIP-340
remote-signing interface exists in the Logos stack.**

**Fallback, mandatory for B1 and practical for L1:** a repository-owned signer
daemon (`lez-swap-signer`) holding both chains' keys behind typed JSON-RPC on
an owner-only Unix socket, keys in an OS-keystore/HSM-wrapped envelope.
Hardware wallets attach later for B2/B4/B5 only.

## 4. Architecture

Traits, one per chain, in the SDK crates, shaped like `ZebraClaimSigner`:

```rust
// crates/adaptor-signature: ADR 0213 §8's eight ceremony operations
trait AdaptorSessionSigner { reserve; accept_peer_commitment; reveal_nonce;
    accept_peer_nonce_and_sign; replay_partial; accept_peer_partial; adapt; extract }
// crates/btc-swap-sdk
trait BitcoinSwapSigner { sign_refund_tapleaf(sighash); sign_agreement(commitment); public_keys() }
// crates/lez-bridge-client; the sidecar consumes it via a new --signer-socket
trait LezTransactionSigner { sign_message(exact_message_bytes) -> (Sig, PubKey); account_id() }
// crates/node-common
trait DeliverySigner { sign_offer_digest; sign_announcement_digest }
```

`FileKeySigner` is today's code. `RemoteSigner` speaks
JSON-RPC 2.0 over `/run/lez/<role>/signer.sock` (mode 0600, `SO_PEERCRED` uid
check); each request carries `swap_id`, `context_binding`, a monotonic
`sequence` and an HMAC under a per-swap capability minted at spawn, like the
sidecar's `capability-file`.

State split: the signer owns long-term keys, secret nonces, the Taker's t, the
refund key and its own encrypted journal (today's `adaptor_session_journal`
moves there, closing TM-0002's plaintext-nonce gap). The Node
keeps agreements, commitments, public nonces, presignatures, actor state and
Chat round records. Presigned refunds stay in the Node (ADR 0044, INV-03), so
recovery never depends on the signer. ADR 0034's gate becomes "challenge
the signer for public keys, re-verify retained presignatures".

Approval: the signer keeps a pending-intent queue; the Basecamp desk backends
(`apps/basecamp/{maker,taker}/src/lez_atomic_swap_*_backend.cpp`) surface it
through new owner methods `signer_intent_list_v1` and
`signer_intent_approve_v1` (request-id idempotent, generation-fenced like
`taker_swap_claim_v1`). One approval per swap at agreement time covers B1, B5
and the presigned refund; post-lock claims and L1 funding run under that
standing approval (INV-03); B6 uses a delegated session key.

`docs/api/README.md`: `maker_health`/`taker_health` gain a `signer` dependency
state; the intent methods are documented; `taker_swap_initiate_v1` may return
`signer_approval_pending`; signer-socket access is a stronger grant than the
owner socket. Threat model: TM-0001 (signer is its own uid),
TM-0002 (keys and nonces leave the Node; residual: signer host),
TM-0009/TM-0016 (nonce reuse enforced in the signer journal), TM-0011 (signer
outage after lock, mitigated by presigned refunds), new TM-0030 "signer socket
compromise or intent substitution".

## 5. Phased plan

| PR | Scope | Acceptance on the local stack (`deploy/`) |
|---|---|---|
| 1 | Traits plus `FileKeySigner`; Nodes, actor and sidecar call through them; no behaviour change | `node-e2e.py all` green; presignatures byte-identical to `main` in `two_role_ceremony` |
| 2 | `lez-swap-signer` binary, RPC schema, capability/HMAC, encrypted journal; `RemoteSigner` for `LezTransactionSigner` first (S3.2 order) | `node-e2e.py happy` with a signer container; kill the signer mid-escrow → `signer_unavailable`, then resumes; sidecar argv carries no key path |
| 3 | `RemoteSigner` for agreement, refund and Delivery | `replay`, `wrong-inputs`: mutated intents rejected by the signer; `taker-refund`/`maker-refund` pass with the signer stopped after the lock |
| 4 | Adaptor sessions remoted; nonce journal moves into the signer | `restart-taker`, `restart-maker`, `concurrent`: kill the signer after every round → no nonce reuse, replay returns the stored answer; `survivor` |
| 5 | Intent queue, Basecamp approval UI, API and threat-model updates | `swap-through-ui.sh` with an approval step; `check-threat-model.py` passes |
| 6 | Optional: Bitcoin Core or Ledger for B4 (PSBT); hardware-backed `sign_refund_tapleaf` | Funding via the external wallet on the local stack |

Risks. Nonce reuse if signer journal and Node round records diverge: the signer
is the sole nonce authority; the Node stores only public material. Hardware
interaction windows: three rounds inside 25 s Chat waits rule out push-button
devices, so hardware is confined to non-interactive pre-lock signatures and B1
stays in the daemon. Replay after a Node restart: the signer answers only an
identical `context_binding` and `sequence`, conflicts otherwise, and never
re-arms a consumed session. Approval latency against `offer_ttl_seconds` and
`maker_second_lock_cutoff`: approvals are requested at take time and surfaced
through the Taker's `initiating` state.
