#![no_main]

mod chat_frame_harness;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| chat_frame_harness::run(data));
