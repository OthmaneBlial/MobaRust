#![no_main]

use libfuzzer_sys::fuzz_target;
use mobarust_core::SessionRecord;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<SessionRecord>(data);
});
