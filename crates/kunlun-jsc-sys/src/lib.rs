//! Raw bindings generated from Kunlun's authoritative C ABI header.
//!
//! This crate is intentionally unsafe and contains no ownership abstractions.
//! Ordinary builds generate only from `include/kunlun_jsc.h` and never fetch a
//! native engine artifact.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[cfg(all(test, target_os = "macos"))]
mod tests {
    unsafe extern "C" {
        fn kunlun_jsc_internal_test_bad_alloc_boundary() -> super::kunlun_jsc_status;
        fn kunlun_jsc_internal_test_unknown_exception_boundary() -> super::kunlun_jsc_status;
    }

    #[test]
    fn contains_cpp_exceptions() {
        // SAFETY: these private test probes take no inputs and execute the same
        // non-throwing exception guard used by every public shim entry point.
        unsafe {
            assert_eq!(
                kunlun_jsc_internal_test_bad_alloc_boundary(),
                super::KUNLUN_JSC_STATUS_OUT_OF_MEMORY
            );
            assert_eq!(
                kunlun_jsc_internal_test_unknown_exception_boundary(),
                super::KUNLUN_JSC_STATUS_CPP_EXCEPTION
            );
        }
    }
}
