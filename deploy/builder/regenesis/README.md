# regenesis

`regenesis` computes the genesis values that `scripts/gen-config.sh` writes into the
devnet's Bedrock deployment settings: the genesis block root and the SDP
declaration's locked note id. It recomputes them after setting every genesis note of
one key to a new value. Both depend on the genesis transfer, so a node refuses a
genesis whose note values were edited without them. It starts no node and logs
nothing.

It is built against logos-blockchain 0.2.4 (the node `blockchain_module` 0.2.4
wraps) as a member of that workspace, in a container:

```sh
git clone --depth 1 --branch 0.2.4 https://github.com/logos-blockchain/logos-blockchain.git lb
mkdir -p lb/tools/regenesis/src
cp deploy/builder/regenesis/main.rs lb/tools/regenesis/src/main.rs
printf '[package]\nname = "regenesis"\nversion = "0.1.0"\nedition = "2024"\npublish = false\n\n[dependencies]\nlb-core = { workspace = true }\nserde_yml = { workspace = true }\n' >lb/tools/regenesis/Cargo.toml
sed -i 's|^members = \[|members = [\n  "tools/regenesis",|' lb/Cargo.toml
cp deploy/assets/lez-source/bedrock/deployment-settings.yaml lb/upstream.yaml
docker run --rm -v "$PWD/lb:/src" -w /src rust:1.96.0-bookworm \
  sh -c 'cargo build -p regenesis && ./target/debug/regenesis upstream.yaml \
    2e03b2eff5a45478e7e79668d2a146cf2c5c7925bce927f2b1c67f2ab4fc0d26 1000000000000'
```

It prints the block root and the locked note id as byte lists (`- 247`, …); in hex
they are the `devnet_genesis_root` and `devnet_locked_note` constants in
gen-config.sh. Given the upstream values, it reproduces upstream's root
(`cb5951ac…`) and locked note (`7e449a14…`).

Build time: about 4 minutes on 2 CPUs.
