# Provenance: the bundle's Monero tests, 0.4.1 candidate

Every test in this set ran on the candidate release bundle of `main` at `2373e28735b732d2183bb4ce2b494c95ff8d42a1`, the commit tagged `v0.4.1`.

- **Bundle:** `lez-swap-stack-sha-2373e28735b7-arm64.tar.gz`, artifact `lez-swap-stack-bundle` of GitHub run 45887200123 (`release-images.yml` on `main`), sha256 `64cc333eff1d3702b9db9feb9843201955e3eebec430a9a1c5972d2150c2a31d`.
- **`release.env`:** `LEZ_IMAGE_TAG=sha-2373e28735b7`, `LEZ_RELEASE_COMMIT=2373e28735b732d2183bb4ce2b494c95ff8d42a1`.
- **Stack:** `LEZ_STACK_PREFIX=lez041n LEZ_MONERO=1 LEZ_TIMING_PROFILE=fast ./scripts/start.sh` on an Apple-silicon Mac. The same stack then ran the native macOS swaps of [macos-native-0.4.1-2026-11-27](../macos-native-0.4.1-2026-11-27/README.md).
- **Role images:** the build source read from each image's `/usr/local/share/lez/build-source.txt` (recorded in [`B-provenance.txt`](../macos-native-0.4.1-2026-11-27/logs/B-provenance.txt)).

| Image | Build source | Digest |
|---|---|---|
| `lez-maker-node:sha-2373e28735b7` | `2373e28735b732d2183bb4ce2b494c95ff8d42a1` | `sha256:5ae98bfb80d5afa39bf7cb03390306975148cb9458fa7b86bb2817d00b300480` |
| `lez-taker-node:sha-2373e28735b7` | `2373e28735b732d2183bb4ce2b494c95ff8d42a1` | `sha256:8504d90c08ace6813485555862b54948230c2b647bee4b054fe62da95fca6885` |
| `lez-monerod` | Monero `v0.18.5.1` | `sha256:fdb240113fc3a795e820f0f13eaac1b331a99044bab4c811d6b623d7b1c66e3d` |

The other images of the stack and their digests are in the macOS set's [PROVENANCE.md](../macos-native-0.4.1-2026-11-27/PROVENANCE.md).

- **Harness:** the bundle's own `scripts/regress.sh` and `scripts/node-e2e.py`, unchanged.
