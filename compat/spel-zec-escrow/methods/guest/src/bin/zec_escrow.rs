#![no_main]

risc0_zkvm::guest::entry!(main);

#[path = "../../../../src/lib.rs"]
mod contract;

pub fn main() {
    contract::main();
}
