//! Compiles the `windowed_transfer` guest with the RISC Zero Rust toolchain
//! (`rzup install rust`). The guest is test-only and never deployed, so it does
//! not need the reproducible Docker build the escrow artifact uses.

fn main() {
    risc0_build::embed_methods();
}
