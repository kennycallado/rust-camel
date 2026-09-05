#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    camel_fuzz::dsl_template_harness(data);
});
