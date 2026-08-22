use crate::{BackendInfo, HostCall, JscError};
use kunlun_jsc_sys as sys;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::time::Duration;

type SleepScheduler = dyn Fn(Duration, DeferredPromise);
type HostScheduler = dyn Fn(HostCall, DeferredPromise);

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
    raw: sys::JSGlobalContextRef,
}

impl ContextInner {
    fn as_context(&self) -> sys::JSContextRef {
        self.raw.cast_const()
    }

    fn value_to_string(&self, value: sys::JSValueRef) -> Result<String, JscError> {
        let mut exception = ptr::null();
        // SAFETY: `value` belongs to this live context. The returned JS string
        // is owned by `OwnedJsString` and released exactly once.
        let string = unsafe { sys::JSValueToStringCopy(self.as_context(), value, &mut exception) };
        if !exception.is_null() || string.is_null() {
            return Err(JscError::ValueConversion);
        }
        OwnedJsString { raw: string }.to_utf8()
    }
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        // SAFETY: `raw` was created by `JSGlobalContextCreate`, and the Rc
        // ownership model ensures this is its final release.
        unsafe { sys::JSGlobalContextRelease(self.raw) };
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
        // SAFETY: passing a null class requests JSC's default global object.
        let raw = unsafe { sys::JSGlobalContextCreate(ptr::null()) };
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
        unsafe { sys::JSGlobalContextSetName(self.context.raw, name.raw) };
        Ok(())
    }

    pub fn set_inspectable(&self, inspectable: bool) {
        // SAFETY: the owned context is live and accessed only on this thread.
        unsafe { sys::JSGlobalContextSetInspectable(self.context.raw, inspectable) };
    }

    pub fn is_inspectable(&self) -> bool {
        // SAFETY: the owned context is live and accessed only on this thread.
        unsafe { sys::JSGlobalContextIsInspectable(self.context.raw) }
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
        let global = unsafe { sys::JSContextGetGlobalObject(context) };
        // SAFETY: `name` and `context` are live. The callback has the exact C
        // ABI expected by JavaScriptCore.
        let function = unsafe {
            sys::JSObjectMakeFunctionWithCallback(context, name.raw, Some(sleep_callback))
        };
        if global.is_null() || function.is_null() {
            return Err(JscError::HostFunction(
                "could not create the global sleep function".to_owned(),
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context, and no attribute bits
        // are requested.
        unsafe {
            sys::JSObjectSetProperty(
                context,
                global,
                name.raw,
                function.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }

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
        let global = unsafe { sys::JSContextGetGlobalObject(context) };
        // SAFETY: the callback has JSC's exact C ABI and dispatches only
        // through the context-local registry.
        let function = unsafe {
            sys::JSObjectMakeFunctionWithCallback(context, name.raw, Some(host_call_callback))
        };
        if global.is_null() || function.is_null() {
            return Err(JscError::HostFunction(
                "could not create the generic Kunlun host-call function".to_owned(),
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context.
        unsafe {
            sys::JSObjectSetProperty(
                context,
                global,
                name.raw,
                function.cast_const(),
                sys::K_JS_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }

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
        // SAFETY: all handles belong to this live context; null `thisObject`
        // requests the global object and JSC writes at most one exception.
        let value = unsafe {
            sys::JSEvaluateScript(
                self.context.as_context(),
                source.raw,
                ptr::null_mut(),
                source_url.raw,
                1,
                &mut exception,
            )
        };

        if !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
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
    resolve: sys::JSObjectRef,
    reject: sys::JSObjectRef,
}

impl DeferredPromise {
    fn new(context: Rc<ContextInner>) -> Result<(sys::JSObjectRef, Self), JscError> {
        let mut resolve = ptr::null_mut();
        let mut reject = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: JSC initializes the promise and resolver outputs for this
        // live context. The resolver functions are protected below.
        let promise = unsafe {
            sys::JSObjectMakeDeferredPromise(
                context.as_context(),
                &mut resolve,
                &mut reject,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(JscError::Exception(context.value_to_string(exception)?));
        }
        if promise.is_null() || resolve.is_null() || reject.is_null() {
            return Err(JscError::PromiseCreation);
        }

        // SAFETY: both functions belong to this context. Protection is paired
        // with `JSValueUnprotect` in Drop.
        unsafe {
            sys::JSValueProtect(context.as_context(), resolve.cast_const());
            sys::JSValueProtect(context.as_context(), reject.cast_const());
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
        // SAFETY: undefined is created in the same live context.
        let value = unsafe { sys::JSValueMakeUndefined(self.context.as_context()) };
        self.settle(self.resolve, value)
    }

    pub fn resolve_string(self, value: &str) -> Result<(), JscError> {
        let value = OwnedJsString::new(value)?;
        // SAFETY: the JS string and resulting value belong to the same context.
        let value = unsafe { sys::JSValueMakeString(self.context.as_context(), value.raw) };
        self.settle(self.resolve, value)
    }

    pub fn reject_message(self, message: &str) -> Result<(), JscError> {
        let message = OwnedJsString::new(message)?;
        // SAFETY: the JS string and resulting value belong to the same context.
        let value = unsafe { sys::JSValueMakeString(self.context.as_context(), message.raw) };
        self.settle(self.reject, value)
    }

    fn settle(&self, function: sys::JSObjectRef, value: sys::JSValueRef) -> Result<(), JscError> {
        let arguments = [value];
        let mut exception = ptr::null();
        // SAFETY: the protected resolver and argument belong to the same live
        // context. JSC invokes the Promise reactions before returning control.
        unsafe {
            sys::JSObjectCallAsFunction(
                self.context.as_context(),
                function,
                ptr::null_mut(),
                arguments.len(),
                arguments.as_ptr(),
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(JscError::Exception(
                self.context.value_to_string(exception)?,
            ));
        }
        Ok(())
    }
}

impl Drop for DeferredPromise {
    fn drop(&mut self) {
        // SAFETY: protection was established in `new`, and the retained Rc
        // guarantees the context is still alive.
        unsafe {
            sys::JSValueUnprotect(self.context.as_context(), self.resolve.cast_const());
            sys::JSValueUnprotect(self.context.as_context(), self.reject.cast_const());
        }
    }
}

unsafe extern "C" fn sleep_callback(
    context: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    let hook = SLEEP_HOOKS.with(|hooks| hooks.borrow().get(&(context as usize)).cloned());
    let Some(hook) = hook else {
        set_callback_exception(
            context,
            exception,
            "sleep() called outside a Kunlun event loop",
        );
        return ptr::null();
    };

    let milliseconds = if argument_count == 0 {
        0.0
    } else if arguments.is_null() {
        set_callback_exception(
            context,
            exception,
            "sleep() received an invalid argument list",
        );
        return ptr::null();
    } else {
        let mut conversion_exception = ptr::null();
        // SAFETY: JSC guarantees at least `argument_count` entries when the
        // argument pointer is non-null.
        let argument = unsafe { *arguments };
        // SAFETY: the argument belongs to the callback context.
        let number = unsafe { sys::JSValueToNumber(context, argument, &mut conversion_exception) };
        if !conversion_exception.is_null() {
            if !exception.is_null() {
                // SAFETY: the caller provided writable exception storage.
                unsafe { *exception = conversion_exception };
            }
            return ptr::null();
        }
        number
    };

    let Ok(duration) = Duration::try_from_secs_f64(milliseconds / 1_000.0) else {
        set_callback_exception(
            context,
            exception,
            "sleep(milliseconds) requires a finite, non-negative duration",
        );
        return ptr::null();
    };

    let (promise, deferred) = match DeferredPromise::new(Rc::clone(&hook.context)) {
        Ok(result) => result,
        Err(error) => {
            set_callback_exception(context, exception, &error.to_string());
            return ptr::null();
        }
    };

    if catch_unwind(AssertUnwindSafe(|| (hook.schedule)(duration, deferred))).is_err() {
        set_callback_exception(context, exception, "Kunlun timer scheduler panicked");
        return ptr::null();
    }

    promise.cast_const()
}

unsafe extern "C" fn host_call_callback(
    context: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this_object: sys::JSObjectRef,
    argument_count: usize,
    arguments: *const sys::JSValueRef,
    exception: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    let hook = HOST_HOOKS.with(|hooks| hooks.borrow().get(&(context as usize)).cloned());
    let Some(hook) = hook else {
        set_callback_exception(
            context,
            exception,
            "Kunlun host call used outside a registered runtime",
        );
        return ptr::null();
    };
    if argument_count < 2 || arguments.is_null() {
        set_callback_exception(
            context,
            exception,
            "Kunlun host call requires operation and JSON payload strings",
        );
        return ptr::null();
    }

    // SAFETY: JSC guarantees `argument_count` callback arguments.
    let values = unsafe { std::slice::from_raw_parts(arguments, argument_count) };
    let operation = match hook.context.value_to_string(values[0]) {
        Ok(value) => value,
        Err(error) => {
            set_callback_exception(context, exception, &error.to_string());
            return ptr::null();
        }
    };
    let payload = match hook.context.value_to_string(values[1]) {
        Ok(value) => value,
        Err(error) => {
            set_callback_exception(context, exception, &error.to_string());
            return ptr::null();
        }
    };

    let (promise, deferred) = match DeferredPromise::new(Rc::clone(&hook.context)) {
        Ok(result) => result,
        Err(error) => {
            set_callback_exception(context, exception, &error.to_string());
            return ptr::null();
        }
    };
    let call = HostCall { operation, payload };
    if catch_unwind(AssertUnwindSafe(|| (hook.schedule)(call, deferred))).is_err() {
        set_callback_exception(context, exception, "Kunlun host scheduler panicked");
        return ptr::null();
    }

    promise.cast_const()
}

fn set_callback_exception(
    context: sys::JSContextRef,
    exception: *mut sys::JSValueRef,
    message: &str,
) {
    if exception.is_null() {
        return;
    }
    let Ok(message) = OwnedJsString::new(message) else {
        return;
    };
    // SAFETY: the caller supplied writable exception storage and the string is
    // live for the value creation call.
    unsafe {
        *exception = sys::JSValueMakeString(context, message.raw);
    }
}

struct OwnedJsString {
    raw: sys::JSStringRef,
}

impl OwnedJsString {
    fn new(value: &str) -> Result<Self, JscError> {
        let value = CString::new(value).map_err(|_| JscError::InvalidString)?;
        // SAFETY: CString guarantees a valid NUL-terminated input.
        let raw = unsafe { sys::JSStringCreateWithUTF8CString(value.as_ptr()) };
        if raw.is_null() {
            return Err(JscError::ValueConversion);
        }
        Ok(Self { raw })
    }

    fn to_utf8(&self) -> Result<String, JscError> {
        // SAFETY: `raw` is a live owned JS string.
        let capacity = unsafe { sys::JSStringGetMaximumUTF8CStringSize(self.raw) };
        if capacity == 0 {
            return Err(JscError::ValueConversion);
        }

        let mut buffer = vec![0_u8; capacity];
        // SAFETY: the buffer has the capacity reported by JSC and is writable.
        let written = unsafe {
            sys::JSStringGetUTF8CString(self.raw, buffer.as_mut_ptr().cast::<c_char>(), capacity)
        };
        if written == 0 {
            return Err(JscError::ValueConversion);
        }

        // SAFETY: a successful JSC conversion writes a terminating NUL.
        let c_string = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) };
        c_string
            .to_str()
            .map(str::to_owned)
            .map_err(|_| JscError::ValueConversion)
    }
}

impl Drop for OwnedJsString {
    fn drop(&mut self) {
        // SAFETY: `raw` is owned by this wrapper and released exactly once.
        unsafe { sys::JSStringRelease(self.raw) };
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
}
