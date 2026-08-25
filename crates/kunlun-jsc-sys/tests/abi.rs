use kunlun_jsc_sys as sys;
use std::mem::size_of;
#[cfg(target_os = "macos")]
use std::mem::size_of_val;

#[test]
fn generated_layout_matches_fixed_width_abi() {
    assert_eq!(size_of::<sys::kunlun_jsc_status>(), 4);
    assert_eq!(size_of::<sys::kunlun_jsc_property_attributes>(), 4);
    assert_eq!(
        size_of::<sys::kunlun_jsc_function_callback>(),
        size_of::<usize>()
    );
    assert_eq!(sys::KUNLUN_JSC_ABI_VERSION, 1);
    assert_eq!(sys::KUNLUN_JSC_STATUS_OK, 0);
    assert_eq!(sys::KUNLUN_JSC_STATUS_CPP_EXCEPTION, 7);
}

#[cfg(target_os = "macos")]
#[test]
fn links_every_allowlisted_symbol() {
    let symbols = [
        sys::kunlun_jsc_context_create as *const (),
        sys::kunlun_jsc_context_release as *const (),
        sys::kunlun_jsc_context_get_global_object as *const (),
        sys::kunlun_jsc_context_set_name as *const (),
        sys::kunlun_jsc_context_set_inspectable as *const (),
        sys::kunlun_jsc_context_is_inspectable as *const (),
        sys::kunlun_jsc_string_create_utf8 as *const (),
        sys::kunlun_jsc_string_get_max_utf8_size as *const (),
        sys::kunlun_jsc_string_write_utf8 as *const (),
        sys::kunlun_jsc_string_release as *const (),
        sys::kunlun_jsc_evaluate as *const (),
        sys::kunlun_jsc_object_call_as_function as *const (),
        sys::kunlun_jsc_object_make_deferred_promise as *const (),
        sys::kunlun_jsc_object_make_function as *const (),
        sys::kunlun_jsc_object_set_property as *const (),
        sys::kunlun_jsc_value_make_string as *const (),
        sys::kunlun_jsc_value_make_undefined as *const (),
        sys::kunlun_jsc_value_to_number as *const (),
        sys::kunlun_jsc_value_to_string as *const (),
        sys::kunlun_jsc_value_protect as *const (),
        sys::kunlun_jsc_value_unprotect as *const (),
    ];

    assert_eq!(symbols.len(), 21);
    assert_eq!(size_of_val(&symbols), 21 * size_of::<usize>());
    assert!(symbols.iter().all(|symbol| !symbol.is_null()));
}

#[cfg(target_os = "macos")]
#[test]
fn compiles_calls_and_releases_the_c_abi() {
    use std::ptr;

    let mut context = ptr::null_mut();
    // SAFETY: every output pointer is writable, every borrowed handle stays
    // within the live context, and each owned handle is released exactly once.
    unsafe {
        assert_eq!(
            sys::kunlun_jsc_context_create(&mut context),
            sys::KUNLUN_JSC_STATUS_OK
        );
        assert!(!context.is_null());

        let source_bytes = b"21 * 2";
        let mut source = ptr::null_mut();
        assert_eq!(
            sys::kunlun_jsc_string_create_utf8(
                source_bytes.as_ptr(),
                source_bytes.len() as u64,
                &mut source,
            ),
            sys::KUNLUN_JSC_STATUS_OK
        );

        let url_bytes = b"test:///abi-smoke.js";
        let mut source_url = ptr::null_mut();
        assert_eq!(
            sys::kunlun_jsc_string_create_utf8(
                url_bytes.as_ptr(),
                url_bytes.len() as u64,
                &mut source_url,
            ),
            sys::KUNLUN_JSC_STATUS_OK
        );

        let mut result = ptr::null();
        let mut exception = ptr::null();
        assert_eq!(
            sys::kunlun_jsc_evaluate(
                context,
                source,
                ptr::null_mut(),
                source_url,
                1,
                &mut result,
                &mut exception,
            ),
            sys::KUNLUN_JSC_STATUS_OK
        );
        assert!(!result.is_null());
        assert!(exception.is_null());

        let mut number = 0.0;
        assert_eq!(
            sys::kunlun_jsc_value_to_number(context, result, &mut number, &mut exception,),
            sys::KUNLUN_JSC_STATUS_OK
        );
        assert_eq!(number, 42.0);

        assert_eq!(
            sys::kunlun_jsc_string_release(source_url),
            sys::KUNLUN_JSC_STATUS_OK
        );
        assert_eq!(
            sys::kunlun_jsc_string_release(source),
            sys::KUNLUN_JSC_STATUS_OK
        );
        assert_eq!(
            sys::kunlun_jsc_context_release(context),
            sys::KUNLUN_JSC_STATUS_OK
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn reports_invalid_inputs_with_status_values() {
    use std::ptr;

    // SAFETY: null is passed deliberately to exercise the documented
    // validation path; the shim checks it before dereferencing.
    unsafe {
        assert_eq!(
            sys::kunlun_jsc_context_create(ptr::null_mut()),
            sys::KUNLUN_JSC_STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            sys::kunlun_jsc_string_create_utf8(ptr::null(), 1, ptr::null_mut()),
            sys::KUNLUN_JSC_STATUS_INVALID_ARGUMENT
        );
    }
}
