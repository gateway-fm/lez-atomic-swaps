#![no_main]

mod xmr_reserve_wire_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| xmr_reserve_wire_harness::run(data));
