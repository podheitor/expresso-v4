#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    expresso_drive::fuzz_entry::fuzz_text_extract(data);
});
