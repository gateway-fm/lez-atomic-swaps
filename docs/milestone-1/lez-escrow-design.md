# LEZ escrow and SPEL IDL sketch

Status: draft; current SPEL/LEZ compatibility verification is open — 2026-07-11

## Account model

One escrow PDA is derived from the program ID and a collision-resistant swap ID.
It stores protocol version, pair, direction, asset definition/native marker, amount, maker
refund authority, taker claim authority/commitment, claim mode, creation point,
LEZ refund deadline, foreign-chain commitment digest, and terminal status.

Funds move through the native-token or associated-token-account program rather
than bespoke balance code. Initialization claims the fresh PDA and transfers the
exact negotiated amount. Terminal transitions are single-use.

## Instruction sketch

    #[lez_program(instruction = "lez_swap_escrow_core::Instruction")]
    mod lez_swap_escrow {
        #[instruction]
        fn initialize(/* fresh escrow PDA, funding ATA/native account, terms */);

        #[instruction]
        fn claim_hashlock(/* escrow, taker destination, token accounts, preimage */);

        #[instruction]
        fn claim_adaptor(/* escrow, taker destination, token accounts, signer */);

        #[instruction]
        fn refund(/* escrow, maker destination, token accounts, clock */);
    }

`initialize` rejects an existing PDA, zero amount, unsupported pair/mode,
unsafe deadline, asset mismatch, or missing maker authorization.
`claim_hashlock` applies only to ZEC and verifies SHA-256(preimage).
`claim_adaptor` applies to BTC/XMR and requires the expected validated signer
authorization; exact witness/public-key exposure is an upstream verification
gate. `refund` requires the maker authority and the LEZ deadline to have passed.

## IDL/account types

The generated SPEL IDL must include `EscrowState`, `Pair`, `ClaimMode`,
`EscrowStatus`, and every instruction/account constraint. Golden IDL output and
generated client compilation are CI tests.

## Validity-window use

LEZ source currently enforces lower-inclusive, upper-exclusive windows during
block construction. Claim and refund transactions use disjoint validity ranges
where feasible. Pair-specific external deadlines remain strictly later than the
LEZ refund deadline plus measured inclusion/reorg margin.

## Open compatibility issue

SPEL v0.5 documentation still shows older `nssa` paths while current LEZ `dev`
uses `lee/state_machine`. No implementation dependency is pinned until a minimal
generated program builds and runs against the same LEZ commit used by the
standalone-sequencer tests.
