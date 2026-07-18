# M3 official BIP vector provenance

These files are unmodified inputs from the Bitcoin BIPs repository at immutable
commit `8c369ac8e60629ac6c032ffe21bb5ec5b35213d7`:

- repository: `https://github.com/bitcoin/bips`
- BIP-327 vectors: `bip-0327/vectors/*.json`
- BIP-340 vectors: `bip-0340/test-vectors.csv`
- BIP-340 license: `bip-0340/LICENSE`

`SHA256SUMS` binds every vendored upstream file. The repository checker rejects
missing, additional, malformed, symlinked, or hash-mismatched inputs.

Licensing at the pinned source revision:

- BIP-327: BSD-3-Clause. The authoritative declaration is in
  `bip-0327.mediawiki`, whose SHA-256 is
  `b79a9e3cc9a23a91f6b1d7cc9cb35b2af6989c95426f7069d3efd5af7f0bf913`.
- BIP-340: BSD-2-Clause. The exact upstream license text is retained at
  `bip-0340/LICENSE` and bound by `SHA256SUMS`.

The files are test inputs only. They do not enter a runtime artifact and do not
replace the separately pinned production cryptographic dependencies.
