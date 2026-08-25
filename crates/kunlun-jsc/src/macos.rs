use crate::{BackendInfo, HostCall, JscError};
use kunlun_jsc_sys as sys;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::time::Duration;

type SleepScheduler = dyn Fn(Duration, DeferredPromise);
type HostScheduler = dyn Fn(HostCall, DeferredPromise);
type ContextRef = *mut sys::kunlun_jsc_context;
type ObjectRef = *mut sys::kunlun_jsc_object;
type ValueRef = *const sys::kunlun_jsc_value;

fn expect_status(operation: &'static str, status: sys::kunlun_jsc_status) -> Result<(), JscError> {
    if status == sys::KUNLUN_JSC_STATUS_OK {
        Ok(())
    } else {
        Err(JscError::NativeStatus { operation, status })
    }
}

#[derive(Clone)]
struct SleepHook {
    context: Rc<ContextInner>,
    schedule: Rc<SleepScheduler>,
}

#[derive(Clone)]
struct HostHook {
    context: Rc<ContextInner>,
    schedule: Rc<HostScheduler>,
}

thread_local! {
    static SLEEP_HOOKS: RefCell<HashMap<usize, SleepHook>> = RefCell::new(HashMap::new());
    static HOST_HOOKS: RefCell<HashMap<usize, HostHook>> = RefCell::new(HashMap::new());
}

struct ContextInner {
    raw: ContextRef,
}

impl ContextInner {
    fn as_context(&self) -> ContextRef {
        self.raw
    }

    fn value_to_string(&self, value: ValueRef) -> Result<String, JscError> {
        let mut string = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: `value` belongs to this live context. The returned JS string
        // is owned by `OwnedJsString` and released exactly once.
        let status = unsafe {
            sys::kunlun_jsc_value_to_string(self.as_context(), value, &mut string, &mut exception)
        };
        if status != sys::KUNLUN_JSC_STATUS_OK || !exception.is_null() || string.is_null() {
            return Err(JscError::ValueConversion);
        }
        OwnedJsString { raw: string }.to_utf8()
    }
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        // SAFETY: `raw` was created by `JSGlobalContextCreate`, and the Rc
        // ownership model ensures this is its final release.
        let _ = unsafe { sys::kunlun_jsc_context_release(self.raw) };
    }
}

/// An owned, thread-affine JavaScriptCore global context.
pub struct JscVm {
    context: Rc<ContextInner>,
}

impl JscVm {
    pub const fn backend_info() -> BackendInfo {
        BackendInfo {
            name: "JavaScriptCore",
            distribution: "macOS system framework (bootstrap only)",
            hermetic: false,
            supports_inspection: true,
            supports_deferred_promises: true,
            supports_native_modules: false,
            supports_explicit_microtask_checkpoint: false,
        }
    }

    pub fn new(name: &str) -> Result<Self, JscError> {
        let mut raw = ptr::null_mut();
        // SAFETY: `raw` is writable output storage and the shim creates the
        // default global context without exposing JSC implementation types.
        let status = unsafe { sys::kunlun_jsc_context_create(&mut raw) };
        expect_status("context_create", status)?;
        if raw.is_null() {
            return Err(JscError::ContextCreation);
        }

        let vm = Self {
            context: Rc::new(ContextInner { raw }),
        };
        vm.set_name(name)?;
        Ok(vm)
    }

    pub fn set_name(&self, name: &str) -> Result<(), JscError> {
        let name = OwnedJsString::new(name)?;
        // SAFETY: both the context and name are live for the duration of the
        // call; JSC copies the context name.
        let status = unsafe { sys::kunlun_jsc_context_set_name(self.context.raw, name.raw) };
        expect_status("context_set_name", status)
    }

    pub fn set_inspectable(&self, inspectable: bool) {
        // SAFETY: the owned context is live and accessed only on this thread.
        let status = unsafe {
            sys::kunlun_jsc_context_set_inspectable(self.context.raw, u8::from(inspectable))
        };
        debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
    }

    pub fn is_inspectable(&self) -> bool {
        let mut inspectable = 0;
        // SAFETY: the owned context is live and accessed only on this thread.
        let status =
            unsafe { sys::kunlun_jsc_context_is_inspectable(self.context.raw, &mut inspectable) };
        debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
        status == sys::KUNLUN_JSC_STATUS_OK && inspectable != 0
    }

    /// Installs `sleep(milliseconds)` as a Promise-returning host function.
    ///
    /// The scheduler is runtime-agnostic. Kunlun Runtime supplies a Tokio
    /// current-thread scheduler, while this binding only owns JSC handles.
    pub fn install_sleep_scheduler<F>(&self, schedule: F) -> Result<(), JscError>
    where
        F: Fn(Duration, DeferredPromise) + 'static,
    {
        let name = OwnedJsString::new("sleep")?;
        let context = self.context.as_context();

        // SAFETY: the callback uses only the context-local registry below, and
        // the global object retains the created function.
        let mut global = ptr::null_mut();
        let status = unsafe { sys::kunlun_jsc_context_get_global_object(context, &mut global) };
        expect_status("context_get_global_object", status)?;
        // SAFETY: `name` and `context` are live. The callback has the exact C
        // ABI expected by JavaScriptCore.
        let mut function = ptr::null_mut();
        let mut function_exception = ptr::null();
        let status = unsafe {
            sys::kunlun_jsc_object_make_function(
                context,
                name.raw,
                Some(sleep_callback),
                &mut function,
                &mut function_exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !function_exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(function_exception)?,
            ));
        }
        expect_status("object_make_function", status)?;
        if global.is_null() || function.is_null() {
            return Err(JscError::HostFunction(
                "could not create the global sleep function".to_owned(),
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context, and no attribute bits
        // are requested.
        let status = unsafe {
            sys::kunlun_jsc_object_set_property(
                context,
                global,
                name.raw,
                function.cast_const(),
                sys::KUNLUN_JSC_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
        expect_status("object_set_property", status)?;

        let hook = SleepHook {
            context: Rc::clone(&self.context),
            schedule: Rc::new(schedule),
        };
        SLEEP_HOOKS.with(|hooks| {
            hooks.borrow_mut().insert(context as usize, hook);
        });
        Ok(())
    }

    /// Installs the generic Promise-returning host-call bridge used by
    /// bootstrap built-in modules.
    pub fn install_host_call_scheduler<F>(&self, schedule: F) -> Result<(), JscError>
    where
        F: Fn(HostCall, DeferredPromise) + 'static,
    {
        let name = OwnedJsString::new("__kunlunHostCall")?;
        let context = self.context.as_context();

        // SAFETY: the global object belongs to this live context.
        let mut global = ptr::null_mut();
        let status = unsafe { sys::kunlun_jsc_context_get_global_object(context, &mut global) };
        expect_status("context_get_global_object", status)?;
        // SAFETY: the callback has JSC's exact C ABI and dispatches only
        // through the context-local registry.
        let mut function = ptr::null_mut();
        let mut function_exception = ptr::null();
        let status = unsafe {
            sys::kunlun_jsc_object_make_function(
                context,
                name.raw,
                Some(host_call_callback),
                &mut function,
                &mut function_exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !function_exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(function_exception)?,
            ));
        }
        expect_status("object_make_function", status)?;
        if global.is_null() || function.is_null() {
            return Err(JscError::HostFunction(
                "could not create the generic Kunlun host-call function".to_owned(),
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_object_set_property(
                context,
                global,
                name.raw,
                function.cast_const(),
                sys::KUNLUN_JSC_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
        expect_status("object_set_property", status)?;

        let hook = HostHook {
            context: Rc::clone(&self.context),
            schedule: Rc::new(schedule),
        };
        HOST_HOOKS.with(|hooks| {
            hooks.borrow_mut().insert(context as usize, hook);
        });
        Ok(())
    }

    pub fn evaluate(&mut self, source: &str, source_url: &str) -> Result<String, JscError> {
        let source = OwnedJsString::new(source)?;
        let source_url = OwnedJsString::new(source_url)?;
        let mut exception = ptr::null();
        let mut value = ptr::null();
        // SAFETY: all handles belong to this live context; null `thisObject`
        // requests the global object and JSC writes at most one exception.
        let status = unsafe {
            sys::kunlun_jsc_evaluate(
                self.context.as_context(),
                source.raw,
                ptr::null_mut(),
                source_url.raw,
                1,
                &mut value,
                &mut exception,
            )
        };

        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
        expect_status("evaluate", status)?;
        if value.is_null() {
            return Err(JscError::ValueConversion);
        }
        self.context.value_to_string(value)
    }
}

impl Drop for JscVm {
    fn drop(&mut self) {
        let key = self.context.as_context() as usize;
        SLEEP_HOOKS.with(|hooks| {
            hooks.borrow_mut().remove(&key);
        });
        HOST_HOOKS.with(|hooks| {
            hooks.borrow_mut().remove(&key);
        });
    }
}

/// A protected pair of JavaScriptCore Promise resolver functions.
///
/// The Rc keeps the context alive until a scheduled local task settles or
/// drops the Promise. This type is deliberately `!Send + !Sync`.
pub struct DeferredPromise {
    context: Rc<ContextInner>,
    resolve: ObjectRef,
    reject: ObjectRef,
}

impl DeferredPromise {
    fn new(context: Rc<ContextInner>) -> Result<(ObjectRef, Self), JscError> {
        let mut promise = ptr::null_mut();
        let mut resolve = ptr::null_mut();
        let mut reject = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: JSC initializes the promise and resolver outputs for this
        // live context. The resolver functions are protected below.
        let status = unsafe {
            sys::kunlun_jsc_object_make_deferred_promise(
                context.as_context(),
                &mut promise,
                &mut resolve,
                &mut reject,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(JscError::Exception(context.value_to_string(exception)?));
        }
        expect_status("object_make_deferred_promise", status)?;
        if promise.is_null() || resolve.is_null() || reject.is_null() {
            return Err(JscError::PromiseCreation);
        }

        // SAFETY: both functions belong to this context. Protection is paired
        // with `JSValueUnprotect` in Drop.
        let status =
            unsafe { sys::kunlun_jsc_value_protect(context.as_context(), resolve.cast_const()) };
        expect_status("value_protect", status)?;
        let status =
            unsafe { sys::kunlun_jsc_value_protect(context.as_context(), reject.cast_const()) };
        if let Err(error) = expect_status("value_protect", status) {
            // SAFETY: the first resolver was protected immediately above.
            let _ = unsafe {
                sys::kunlun_jsc_value_unprotect(context.as_context(), resolve.cast_const())
            };
            return Err(error);
        }
        Ok((
            promise,
            Self {
                context,
                resolve,
                reject,
            },
        ))
    }

    pub fn resolve_undefined(self) -> Result<(), JscError> {
        let mut value = ptr::null();
        // SAFETY: undefined is created in the same live context.
        let status =
            unsafe { sys::kunlun_jsc_value_make_undefined(self.context.as_context(), &mut value) };
        expect_status("value_make_undefined", status)?;
        self.settle(self.resolve, value)
    }

    pub fn resolve_string(self, value: &str) -> Result<(), JscError> {
        let value = OwnedJsString::new(value)?;
        let mut raw_value = ptr::null();
        // SAFETY: the JS string and resulting value belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_value_make_string(self.context.as_context(), value.raw, &mut raw_value)
        };
        expect_status("value_make_string", status)?;
        self.settle(self.resolve, raw_value)
    }

    pub fn reject_message(self, message: &str) -> Result<(), JscError> {
        let message = OwnedJsString::new(message)?;
        let mut value = ptr::null();
        // SAFETY: the JS string and resulting value belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_value_make_string(self.context.as_context(), message.raw, &mut value)
        };
        expect_status("value_make_string", status)?;
        self.settle(self.reject, value)
    }

    fn settle(&self, function: ObjectRef, value: ValueRef) -> Result<(), JscError> {
        let arguments = [value];
        let mut result = ptr::null();
        let mut exception = ptr::null();
        // SAFETY: the protected resolver and argument belong to the same live
        // context. JSC invokes the Promise reactions before returning control.
        let status = unsafe {
            sys::kunlun_jsc_object_call_as_function(
                self.context.as_context(),
                function,
                ptr::null_mut(),
                u32::try_from(arguments.len()).expect("one Promise settlement argument"),
                arguments.as_ptr(),
                &mut result,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
        expect_status("object_call_as_function", status)
    }
}

impl Drop for DeferredPromise {
    fn drop(&mut self) {
        // SAFETY: protection was established in `new`, and the retained Rc
        // guarantees the context is still alive.
        unsafe {
            let _ = sys::kunlun_jsc_value_unprotect(
                self.context.as_context(),
                self.resolve.cast_const(),
            );
            let _ = sys::kunlun_jsc_value_unprotect(
                self.context.as_context(),
                self.reject.cast_const(),
            );
        }
    }
}

unsafe extern "C" fn sleep_callback(
    context: ContextRef,
    _function: ObjectRef,
    _this_object: ObjectRef,
    argument_count: u32,
    arguments: *const ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    match catch_unwind(AssertUnwindSafe(|| {
        sleep_callback_impl(
            context,
            argument_count,
            arguments,
            out_result,
            out_exception,
        )
    })) {
        Ok(status) => status,
        Err(_) => callback_error(context, out_exception, "Kunlun timer callback panicked"),
    }
}

fn sleep_callback_impl(
    context: ContextRef,
    argument_count: u32,
    arguments: *const ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    if out_result.is_null() || out_exception.is_null() {
        return sys::KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: the shim supplies writable callback output storage.
    unsafe {
        *out_result = ptr::null();
        *out_exception = ptr::null();
    }

    let hook = SLEEP_HOOKS.with(|hooks| hooks.borrow().get(&(context as usize)).cloned());
    let Some(hook) = hook else {
        return callback_error(
            context,
            out_exception,
            "sleep() called outside a Kunlun event loop",
        );
    };

    let milliseconds = if argument_count == 0 {
        0.0
    } else if arguments.is_null() {
        return callback_error(
            context,
            out_exception,
            "sleep() received an invalid argument list",
        );
    } else {
        let mut number = 0.0;
        let mut conversion_exception = ptr::null();
        // SAFETY: JSC guarantees at least `argument_count` entries when the
        // argument pointer is non-null.
        let argument = unsafe { *arguments };
        // SAFETY: the argument belongs to the callback context.
        let status = unsafe {
            sys::kunlun_jsc_value_to_number(
                context,
                argument,
                &mut number,
                &mut conversion_exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !conversion_exception.is_null() {
            // SAFETY: the shim supplies writable callback exception storage.
            unsafe { *out_exception = conversion_exception };
            return status;
        }
        if status != sys::KUNLUN_JSC_STATUS_OK {
            return callback_error(
                context,
                out_exception,
                "Kunlun could not convert the sleep duration",
            );
        }
        number
    };

    let Ok(duration) = Duration::try_from_secs_f64(milliseconds / 1_000.0) else {
        return callback_error(
            context,
            out_exception,
            "sleep(milliseconds) requires a finite, non-negative duration",
        );
    };

    let (promise, deferred) = match DeferredPromise::new(Rc::clone(&hook.context)) {
        Ok(result) => result,
        Err(error) => {
            return callback_error(context, out_exception, &error.to_string());
        }
    };

    if catch_unwind(AssertUnwindSafe(|| (hook.schedule)(duration, deferred))).is_err() {
        return callback_error(context, out_exception, "Kunlun timer scheduler panicked");
    }

    // SAFETY: the shim supplies writable callback result storage, and the
    // Promise is borrowed from the live callback context.
    unsafe { *out_result = promise.cast_const() };
    sys::KUNLUN_JSC_STATUS_OK
}

unsafe extern "C" fn host_call_callback(
    context: ContextRef,
    _function: ObjectRef,
    _this_object: ObjectRef,
    argument_count: u32,
    arguments: *const ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    match catch_unwind(AssertUnwindSafe(|| {
        host_call_callback_impl(
            context,
            argument_count,
            arguments,
            out_result,
            out_exception,
        )
    })) {
        Ok(status) => status,
        Err(_) => callback_error(context, out_exception, "Kunlun host callback panicked"),
    }
}

fn host_call_callback_impl(
    context: ContextRef,
    argument_count: u32,
    arguments: *const ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    if out_result.is_null() || out_exception.is_null() {
        return sys::KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: the shim supplies writable callback output storage.
    unsafe {
        *out_result = ptr::null();
        *out_exception = ptr::null();
    }

    let hook = HOST_HOOKS.with(|hooks| hooks.borrow().get(&(context as usize)).cloned());
    let Some(hook) = hook else {
        return callback_error(
            context,
            out_exception,
            "Kunlun host call used outside a registered runtime",
        );
    };
    if argument_count < 2 || arguments.is_null() {
        return callback_error(
            context,
            out_exception,
            "Kunlun host call requires operation and JSON payload strings",
        );
    }

    // SAFETY: JSC guarantees `argument_count` callback arguments.
    let values = unsafe { std::slice::from_raw_parts(arguments, argument_count as usize) };
    let operation = match hook.context.value_to_string(values[0]) {
        Ok(value) => value,
        Err(error) => {
            return callback_error(context, out_exception, &error.to_string());
        }
    };
    let payload = match hook.context.value_to_string(values[1]) {
        Ok(value) => value,
        Err(error) => {
            return callback_error(context, out_exception, &error.to_string());
        }
    };

    let (promise, deferred) = match DeferredPromise::new(Rc::clone(&hook.context)) {
        Ok(result) => result,
        Err(error) => {
            return callback_error(context, out_exception, &error.to_string());
        }
    };
    let call = HostCall { operation, payload };
    if catch_unwind(AssertUnwindSafe(|| (hook.schedule)(call, deferred))).is_err() {
        return callback_error(context, out_exception, "Kunlun host scheduler panicked");
    }

    // SAFETY: the shim supplies writable callback result storage, and the
    // Promise is borrowed from the live callback context.
    unsafe { *out_result = promise.cast_const() };
    sys::KUNLUN_JSC_STATUS_OK
}

fn callback_error(
    context: ContextRef,
    out_exception: *mut ValueRef,
    message: &str,
) -> sys::kunlun_jsc_status {
    set_callback_exception(context, out_exception, message);
    sys::KUNLUN_JSC_STATUS_CALLBACK_ERROR
}

fn set_callback_exception(context: ContextRef, out_exception: *mut ValueRef, message: &str) {
    if out_exception.is_null() {
        return;
    }
    let Ok(message) = OwnedJsString::new(message) else {
        return;
    };
    let mut exception = ptr::null();
    // SAFETY: the caller supplied writable exception storage and the string is
    // live for the value creation call.
    let status = unsafe { sys::kunlun_jsc_value_make_string(context, message.raw, &mut exception) };
    if status == sys::KUNLUN_JSC_STATUS_OK {
        // SAFETY: the shim supplied writable callback exception storage.
        unsafe { *out_exception = exception };
    }
}

struct OwnedJsString {
    raw: *mut sys::kunlun_jsc_string,
}

impl OwnedJsString {
    fn new(value: &str) -> Result<Self, JscError> {
        if value.as_bytes().contains(&0) {
            return Err(JscError::InvalidString);
        }
        let length = u64::try_from(value.len()).map_err(|_| JscError::InvalidString)?;
        let mut raw = ptr::null_mut();
        // SAFETY: the byte slice is live for the call and `raw` is writable
        // output storage. The shim copies the UTF-8 bytes.
        let status =
            unsafe { sys::kunlun_jsc_string_create_utf8(value.as_ptr(), length, &mut raw) };
        expect_status("string_create_utf8", status)?;
        if raw.is_null() {
            return Err(JscError::ValueConversion);
        }
        Ok(Self { raw })
    }

    fn to_utf8(&self) -> Result<String, JscError> {
        let mut capacity = 0_u64;
        // SAFETY: `raw` is a live owned JS string.
        let status = unsafe { sys::kunlun_jsc_string_get_max_utf8_size(self.raw, &mut capacity) };
        expect_status("string_get_max_utf8_size", status)?;
        if capacity == 0 {
            return Err(JscError::ValueConversion);
        }

        let capacity = usize::try_from(capacity).map_err(|_| JscError::ValueConversion)?;
        let mut buffer = vec![0_u8; capacity];
        let mut written = 0_u64;
        // SAFETY: the buffer has the capacity reported by JSC and is writable.
        let status = unsafe {
            sys::kunlun_jsc_string_write_utf8(
                self.raw,
                buffer.as_mut_ptr(),
                u64::try_from(capacity).map_err(|_| JscError::ValueConversion)?,
                &mut written,
            )
        };
        expect_status("string_write_utf8", status)?;
        let written = usize::try_from(written).map_err(|_| JscError::ValueConversion)?;
        if written == 0 || written > buffer.len() || buffer[written - 1] != 0 {
            return Err(JscError::ValueConversion);
        }

        String::from_utf8(buffer[..written - 1].to_vec()).map_err(|_| JscError::ValueConversion)
    }
}

impl Drop for OwnedJsString {
    fn drop(&mut self) {
        // SAFETY: `raw` is owned by this wrapper and released exactly once.
        let _ = unsafe { sys::kunlun_jsc_string_release(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_javascript() {
        let mut vm = JscVm::new("kunlun-test").expect("create VM");
        assert_eq!(vm.evaluate("21 * 2", "test:///eval.js").unwrap(), "42");
    }

    #[test]
    fn returns_javascript_exceptions() {
        let mut vm = JscVm::new("kunlun-test").expect("create VM");
        let error = vm
            .evaluate("throw new Error('boom')", "test:///exception.js")
            .unwrap_err();
        assert!(matches!(error, JscError::Exception(message) if message.contains("boom")));
    }

    #[test]
    fn toggles_web_inspector_visibility() {
        let vm = JscVm::new("kunlun-inspector-test").expect("create VM");
        assert!(!vm.is_inspectable());
        vm.set_inspectable(true);
        assert!(vm.is_inspectable());
        vm.set_inspectable(false);
        assert!(!vm.is_inspectable());
    }

    #[test]
    fn contains_rust_panics_in_host_callbacks() {
        let mut vm = JscVm::new("kunlun-panic-test").expect("create VM");
        vm.install_sleep_scheduler(|_, _| panic!("test scheduler panic"))
            .expect("install sleep callback");

        let error = vm
            .evaluate("sleep(1)", "test:///callback-panic.js")
            .expect_err("callback panic becomes a JavaScript exception");
        assert!(
            matches!(error, JscError::Exception(message) if message.contains("scheduler panicked"))
        );
    }
}
