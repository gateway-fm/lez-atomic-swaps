#![no_main]

mod coordinator_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| coordinator_harness::run(data));
