#![no_main]

use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let _ = mobarust_remote_desktop::decode_command_frame(data);
    let _ = mobarust_remote_desktop::decode_event_frame(data);
    let _ = mobarust_remote_desktop::decode_credential_frame(data);
});
