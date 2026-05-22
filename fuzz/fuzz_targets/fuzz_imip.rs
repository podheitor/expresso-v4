#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    expresso_mail::fuzz_entry::fuzz_imip(data);
});
