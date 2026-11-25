#![no_main]

mod zec_agreement_wire_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| zec_agreement_wire_harness::run(data));
