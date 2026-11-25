#![no_main]

mod offer_announcement_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| offer_announcement_harness::run(data));
