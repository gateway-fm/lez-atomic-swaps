# SDK trait surface

Status: draft for Logos review — 2026-07-11

## Design rules

- One lifecycle vocabulary; pair-specific evidence and errors remain typed.
- Discovery/negotiation are separate from post-lock recovery.
- Construction is pure where possible; I/O is through node and persistence ports.
- No method advances durable state until its required chain evidence validates.
- Public types are serializable and versioned; secrets are redacted from `Debug`
  and never serialized into logs.
- `SwapDirection` is immutable negotiated data; pair adapters map the taker and
  maker roles to LEZ/foreign legs without changing taker-first ordering.

## Common lifecycle sketch

The following is API design, not yet committed as a compatibility promise:

    pub trait SwapProtocol {
        type ForeignLock;
        type ForeignClaim;
        type ForeignRefund;
        type ClaimWitness: Zeroize + Secrecy;
        type Error: std::error::Error + Send + Sync + 'static;

        fn pair(&self) -> Pair;
        fn validate_terms(&self, terms: &SwapTerms) -> Result<SafetyParameters, Self::Error>;
        fn prepare(&self, terms: SwapTerms) -> Result<PreparedSwap, Self::Error>;
        fn validate_taker_lock(
            &self,
            prepared: &PreparedSwap,
            observation: &Self::ForeignLock,
        ) -> Result<ConfirmedTakerLock, Self::Error>;
        fn build_lez_lock(
            &self,
            prepared: &PreparedSwap,
            confirmed: &ConfirmedTakerLock,
        ) -> Result<LezInstruction, Self::Error>;
        fn extract_claim_witness(
            &self,
            prepared: &PreparedSwap,
            claim: &Self::ForeignClaim,
        ) -> Result<Secret<Self::ClaimWitness>, Self::Error>;
        fn build_foreign_refund(
            &self,
            prepared: &PreparedSwap,
            at: ForeignDeadline,
        ) -> Result<Self::ForeignRefund, Self::Error>;
    }

Offer discovery and negotiation use separate `OfferDiscovery` and
`NegotiationChannel` traits and terminate in an immutable, signed `SwapTerms`
transcript before any lock command.

## Pair implementations

- `BtcLezProtocol`: Taproot output/outpoint, adaptor pre-signature, completed
  BIP-340 signature evidence, CSV refund evidence, Bitcoin height.
- `XmrLezProtocol`: spend/view public keys, encrypted signature/key share,
  cross-curve DLEQ, Monero transaction evidence, Monero height.
- `ZecLezProtocol`: transparent outpoint, BIP-199 redeem script, SHA-256 preimage,
  CLTV refund transaction, expiry height and Zcash consensus branch ID.

## Error contract

Errors distinguish retryable observation lag, terminal malformed evidence,
counterparty protocol violation, local dependency outage, unsafe deadline,
reorged evidence, persistence failure, and operator intervention required.
String-only errors are not part of the SDK boundary.

## Review questions

1. Should downstream Logos modules consume async node traits directly, or only a
   deterministic protocol crate plus a reference coordinator?
2. Which stable Logos Delivery/Chat C APIs must appear in SDK examples?
3. Does Logos prefer one workspace release version or independent per-pair
   semantic versions?
