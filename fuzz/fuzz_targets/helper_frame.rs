#![no_main]

use libfuzzer_sys::fuzz_target;
use mobarust_remote_desktop::{HelperCommand, HelperCredential, HelperEvent};

fuzz_target!(|data: &[u8]| {
    let _ = mobarust_remote_desktop::decode_frame::<HelperCommand>(data);
    let _ = mobarust_remote_desktop::decode_frame::<HelperEvent>(data);
    let _ = mobarust_remote_desktop::decode_frame::<HelperCredential>(data);
});
