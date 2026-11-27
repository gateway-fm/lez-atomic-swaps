#![no_main]

mod coordinator_model;
mod coordinator_store_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| coordinator_store_harness::run(data));
