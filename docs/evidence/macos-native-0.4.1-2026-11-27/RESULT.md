# Result: XMR/LEZ with a native macOS role Node, 0.4.1 candidate (2373e287)

1. Host: an Apple-silicon Mac on macOS 26. Chains and the other role came from the candidate bundle `sha-2373e28735b7`, started as `lez041n` with Monero regtest.
2. The native role came from the CI-built candidate packages (run 45887203856): `lez_maker_node`/`lez_taker_node` 0.4.1 darwin-arm64 (build source `2373e287`), installed in a `logosctl` 0.3.2 session with `lez_swap_node` 0.4.1 and `lez_core` 0.4.1-sign.4.
3. Monero on the Mac: the official `monero-wallet-rpc` 0.18.5.1 for macOS, checked against the signed hash manifest, on 127.0.0.1:18083.
4. Maker native, completed (`3398e572…`): passed in 183 s. The Maker's macOS fund tool paid the Monero; its tag 15 claimed the LEZ.
5. Maker native, Taker refunds (`d59a7957…`): passed in 838 s. The Taker's tag 16 returned its LEZ; the Maker's macOS refund tool swept the Monero back.
6. Maker native, punished (`630e2cd6…`): passed in 1055 s. The Maker's macOS tag 17 took the LEZ after the punish time.
7. Taker native, Maker in Docker, completed (`ad8ae324…`): passed in 146 s.
8. All four passed on their first try, with LEZ settled to the unit and the Monero txids on regtest as listed in `README.md`.
9. The bundle's own Monero suite on the same stack, 22 tests on release images: 21 passed on their first try, and one passed on its rerun (`../bundle-xmr-0.4.1-2026-11-27/RESULT.md`).
10. Rehearsal on the 0.4.0 stack with locally built packages: the same four scenarios passed (`logs/A-*`). No defect was found in the macOS path.
