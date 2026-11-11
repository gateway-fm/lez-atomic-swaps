# Zcash public Testnet lane — BLOCKED

**No swap transaction in this bundle.** It records what the routes reported and why no corridor ran; it is not public-chain swap evidence.

- source `a825b27efedb64a5622462c2d08d4401cf962d99`, workflow run none (manual)
- lane profile `public-zec-testnet-v1`, agreement profile `public-testnet-v1` (branch 5437f330, 10 confirmations, refund 192 blocks)
- evidence kind `public-chain-preflight`; written 2026-11-11T06:00:38Z

## Zcash routes

| route | kind | endpoint | Zebra | chain | tip | next-block branch | supported |
|---|---|---|---|---|---|---|---|
| provider | tatum_testnet_x_api_key | https://zcash-testnet-zebrad.gateway.tatum.io/ | /Zebra:7.0.0/ | test | 4607169 | 77190ad9 | no |

- **provider blocked:** next block on branch 77190ad9 (NU7), the agreement profile public-testnet-v1 signs for 5437f330 (NU6.2)

## LEZ testnet

- start: sequencer https://testnet.lez.logos.co/ channel `0101010101010101010101010101010101010101010101010101010101010101` last block 80689 at 2026-11-11T06:00:12Z
- end: sequencer https://testnet.lez.logos.co/ channel `0101010101010101010101010101010101010101010101010101010101010101` last block 80689 at 2026-11-11T06:00:38Z

## Corridors

None ran.

## Coverage

- happy: False, refund: False, directions: none
- complete corridor on the self-hosted route: False, on the provider route: False
- acceptance complete: **False**

## Interventions

None recorded.

Shielding: not part of atomicity: received transparent TAZ is shielded, if at all, by a separate wallet action.
