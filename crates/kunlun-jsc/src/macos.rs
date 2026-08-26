use crate::ownership::{OwnedHandle, ProtectedHandle};
use crate::{BackendInfo, HostCall, JscError};
use kunlun_jsc_sys as sys;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Duration;

type SleepScheduler = dyn Fn(Duration, DeferredPromise);
type HostScheduler = dyn Fn(HostCall, DeferredPromise);
type ContextGroupRef = *mut sys::kunlun_jsc_context_group;
type ContextRef = *mut sys::kunlun_jsc_context;
type ObjectRef = *mut sys::kunlun_jsc_object;
type ValueRef = *const sys::kunlun_jsc_value;

const EXCEPTION_STRINGIFICATION_FALLBACK: &str =
    "JavaScript exception could not be converted to a string";

fn expect_status(operation: &'static str, status: sys::kunlun_jsc_status) -> Result<(), JscError> {
    if status == sys::KUNLUN_JSC_STATUS_OK {
        Ok(())
    } else {
        Err(JscError::native(operation, status))
    }
}

unsafe fn release_context_group(raw: NonNull<sys::kunlun_jsc_context_group>) {
    // SAFETY: OwnedHandle calls this once for a live owned group.
    let status = unsafe { sys::kunlun_jsc_context_group_release(raw.as_ptr()) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
}

unsafe fn release_context(raw: NonNull<sys::kunlun_jsc_context>) {
    // SAFETY: OwnedHandle calls this once for a live owned context.
    let status = unsafe { sys::kunlun_jsc_context_release(raw.as_ptr()) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
}

unsafe fn release_string(raw: NonNull<sys::kunlun_jsc_string>) {
    // SAFETY: OwnedHandle calls this once for a live owned string.
    let status = unsafe { sys::kunlun_jsc_string_release(raw.as_ptr()) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
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

struct ContextGroupInner {
    handle: OwnedHandle<sys::kunlun_jsc_context_group>,
}

struct ContextInner {
    // Field order encodes native teardown: release the context before its
    // final reference to the owning group.
    handle: OwnedHandle<sys::kunlun_jsc_context>,
    _group: Rc<ContextGroupInner>,
}

impl ContextInner {
    fn as_context(&self) -> ContextRef {
        self.handle.as_ptr()
    }

    fn value_to_string(
        &self,
        value: ValueRef,
        operation: &'static str,
        source_url: Option<&str>,
    ) -> Result<String, JscError> {
        match self.value_to_string_once(value, operation) {
            Ok(string) => Ok(string),
            Err(ValueToStringError::Exception(exception)) => {
                let message = self
                    .value_to_string_once(exception, "exception_to_string")
                    .unwrap_or_else(|_| EXCEPTION_STRINGIFICATION_FALLBACK.to_owned());
                Err(JscError::exception(operation, source_url, message))
            }
            Err(ValueToStringError::Conversion(error)) => Err(match source_url {
                Some(url) => error.with_source_url(url),
                None => error,
            }),
        }
    }

    fn value_to_string_once(
        &self,
        value: ValueRef,
        operation: &'static str,
    ) -> Result<String, ValueToStringError> {
        let mut string = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: `value` belongs to this live context. The returned JS string
        // is owned by `OwnedJsString` and released exactly once.
        let status = unsafe {
            sys::kunlun_jsc_value_to_string(self.as_context(), value, &mut string, &mut exception)
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(ValueToStringError::Exception(exception));
        }
        expect_status(operation, status).map_err(ValueToStringError::Conversion)?;
        if !exception.is_null() {
            return Err(ValueToStringError::Conversion(JscError::missing_value(
                operation,
                "JavaScriptCore returned an exception with an OK status",
            )));
        }
        // SAFETY: a successful value_to_string call transfers one owned
        // JSString handle to the caller.
        let string = unsafe { OwnedHandle::from_raw(string, release_string) }.ok_or_else(|| {
            ValueToStringError::Conversion(JscError::missing_value(
                operation,
                "JavaScriptCore returned no owned string",
            ))
        })?;
        OwnedJsString { handle: string }
            .to_utf8(operation)
            .map_err(ValueToStringError::Conversion)
    }

    fn exception_error(
        &self,
        operation: &'static str,
        source_url: Option<&str>,
        exception: ValueRef,
    ) -> JscError {
        let message = self
            .value_to_string_once(exception, "exception_to_string")
            .unwrap_or_else(|_| EXCEPTION_STRINGIFICATION_FALLBACK.to_owned());
        JscError::exception(operation, source_url, message)
    }
}

enum ValueToStringError {
    Exception(ValueRef),
    Conversion(JscError),
}

/// An owned, thread-affine JavaScriptCore context group.
///
/// Contexts retain their group internally, so dropping this handle before a
/// child context cannot release the native group early.
///
/// ```compile_fail
/// use kunlun_jsc::ContextGroup;
/// fn assert_send<T: Send>() {}
/// assert_send::<ContextGroup>();
/// ```
///
/// ```compile_fail
/// use kunlun_jsc::ContextGroup;
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<ContextGroup>();
/// ```
#[derive(Clone)]
pub struct ContextGroup {
    inner: Rc<ContextGroupInner>,
}

impl ContextGroup {
    pub fn new() -> Result<Self, JscError> {
        let mut raw: ContextGroupRef = ptr::null_mut();
        // SAFETY: `raw` is writable output storage.
        let status = unsafe { sys::kunlun_jsc_context_group_create(&mut raw) };
        expect_status("context_group_create", status)?;
        // SAFETY: success transfers ownership of the non-null group.
        let handle =
            unsafe { OwnedHandle::from_raw(raw, release_context_group) }.ok_or_else(|| {
                JscError::missing_value(
                    "context_group_create",
                    "JavaScriptCore returned no context group",
                )
            })?;
        Ok(Self {
            inner: Rc::new(ContextGroupInner { handle }),
        })
    }

    pub fn create_vm(&self, name: &str) -> Result<JscVm, JscError> {
        let mut raw = ptr::null_mut();
        // SAFETY: the group is live and `raw` is writable output storage.
        let status = unsafe {
            sys::kunlun_jsc_context_create_in_group(self.inner.handle.as_ptr(), &mut raw)
        };
        expect_status("context_create_in_group", status)?;
        // SAFETY: success transfers ownership of the non-null context.
        let handle = unsafe { OwnedHandle::from_raw(raw, release_context) }.ok_or_else(|| {
            JscError::missing_value(
                "context_create_in_group",
                "JavaScriptCore returned no context",
            )
        })?;
        let vm = JscVm {
            context: Rc::new(ContextInner {
                handle,
                _group: Rc::clone(&self.inner),
            }),
        };
        vm.set_name(name)?;
        Ok(vm)
    }
}

/// An owned, thread-affine JavaScriptCore global context.
///
/// ```compile_fail
/// use kunlun_jsc::JscVm;
/// fn assert_send<T: Send>() {}
/// assert_send::<JscVm>();
/// ```
///
/// ```compile_fail
/// use kunlun_jsc::JscVm;
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<JscVm>();
/// ```
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
        ContextGroup::new()?.create_vm(name)
    }

    pub fn set_name(&self, name: &str) -> Result<(), JscError> {
        let name = OwnedJsString::new(name, "context_set_name")?;
        // SAFETY: both the context and name are live for the duration of the
        // call; JSC copies the context name.
        let status =
            unsafe { sys::kunlun_jsc_context_set_name(self.context.as_context(), name.as_ptr()) };
        expect_status("context_set_name", status)
    }

    pub fn set_inspectable(&self, inspectable: bool) -> Result<(), JscError> {
        // SAFETY: the owned context is live and accessed only on this thread.
        let status = unsafe {
            sys::kunlun_jsc_context_set_inspectable(
                self.context.as_context(),
                u8::from(inspectable),
            )
        };
        expect_status("context_set_inspectable", status)
    }

    pub fn is_inspectable(&self) -> Result<bool, JscError> {
        let mut inspectable = 0;
        // SAFETY: the owned context is live and accessed only on this thread.
        let status = unsafe {
            sys::kunlun_jsc_context_is_inspectable(self.context.as_context(), &mut inspectable)
        };
        expect_status("context_is_inspectable", status)?;
        Ok(inspectable != 0)
    }

    /// Installs `sleep(milliseconds)` as a Promise-returning host function.
    ///
    /// The scheduler is runtime-agnostic. Kunlun Runtime supplies a Tokio
    /// current-thread scheduler, while this binding only owns JSC handles.
    ///
    /// ```compile_fail
    /// use kunlun_jsc::DeferredPromise;
    /// use std::time::Duration;
    /// type Callback = dyn Fn(Duration, DeferredPromise);
    /// fn assert_send_sync<T: ?Sized + Send + Sync>() {}
    /// assert_send_sync::<Callback>();
    /// ```
    pub fn install_sleep_scheduler<F>(&self, schedule: F) -> Result<(), JscError>
    where
        F: Fn(Duration, DeferredPromise) + 'static,
    {
        let name = OwnedJsString::new("sleep", "install_sleep_scheduler")?;
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
                name.as_ptr(),
                Some(sleep_callback),
                &mut function,
                &mut function_exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !function_exception.is_null() {
            return Err(self.context.exception_error(
                "install_sleep_scheduler",
                None,
                function_exception,
            ));
        }
        expect_status("object_make_function", status)?;
        if global.is_null() || function.is_null() {
            return Err(JscError::host_function(
                "install_sleep_scheduler",
                "could not create the global sleep function",
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context, and no attribute bits
        // are requested.
        let status = unsafe {
            sys::kunlun_jsc_object_set_property(
                context,
                global,
                name.as_ptr(),
                function.cast_const(),
                sys::KUNLUN_JSC_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(self
                .context
                .exception_error("install_sleep_scheduler", None, exception));
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
        let name = OwnedJsString::new("__kunlunHostCall", "install_host_call_scheduler")?;
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
                name.as_ptr(),
                Some(host_call_callback),
                &mut function,
                &mut function_exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !function_exception.is_null() {
            return Err(self.context.exception_error(
                "install_host_call_scheduler",
                None,
                function_exception,
            ));
        }
        expect_status("object_make_function", status)?;
        if global.is_null() || function.is_null() {
            return Err(JscError::host_function(
                "install_host_call_scheduler",
                "could not create the generic Kunlun host-call function",
            ));
        }

        let mut exception = ptr::null();
        // SAFETY: all handles belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_object_set_property(
                context,
                global,
                name.as_ptr(),
                function.cast_const(),
                sys::KUNLUN_JSC_PROPERTY_ATTRIBUTE_NONE,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(self.context.exception_error(
                "install_host_call_scheduler",
                None,
                exception,
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

    pub fn evaluate(&self, source: &str, source_url: &str) -> Result<String, JscError> {
        self.evaluate_rooted(source, source_url)?.to_string()
    }

    /// Evaluates a script and returns an owned, GC-rooted result tied to this
    /// context's lifetime.
    pub fn evaluate_rooted<'context>(
        &'context self,
        source: &str,
        source_url: &str,
    ) -> Result<RootedValue<'context>, JscError> {
        let source = OwnedJsString::new(source, "evaluate")
            .map_err(|error| error.with_source_url(source_url))?;
        let source_url_handle = OwnedJsString::new(source_url, "evaluate")
            .map_err(|error| error.with_source_url(source_url))?;
        let mut exception = ptr::null();
        let mut value = ptr::null();
        // SAFETY: all handles belong to this live context; null `thisObject`
        // requests the global object and JSC writes at most one exception.
        let status = unsafe {
            sys::kunlun_jsc_evaluate(
                self.context.as_context(),
                source.as_ptr(),
                ptr::null_mut(),
                source_url_handle.as_ptr(),
                1,
                &mut value,
                &mut exception,
            )
        };

        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(self
                .context
                .exception_error("evaluate", Some(source_url), exception));
        }
        expect_status("evaluate", status).map_err(|error| error.with_source_url(source_url))?;
        let protected = ProtectedValue::new(Rc::clone(&self.context), value, "evaluate")
            .map_err(|error| error.with_source_url(source_url))?;
        Ok(RootedValue {
            protected,
            source_url: Some(source_url.to_owned()),
            _owner: PhantomData,
        })
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

unsafe fn protect_value(
    context: &ContextInner,
    raw: NonNull<sys::kunlun_jsc_value>,
) -> Result<(), JscError> {
    // SAFETY: the caller proves `raw` belongs to this live context.
    let status = unsafe { sys::kunlun_jsc_value_protect(context.as_context(), raw.as_ptr()) };
    expect_status("value_protect", status)
}

unsafe fn unprotect_value(context: &ContextInner, raw: NonNull<sys::kunlun_jsc_value>) {
    // SAFETY: ProtectedHandle retains the context and owns one protection
    // count for this value.
    let status = unsafe { sys::kunlun_jsc_value_unprotect(context.as_context(), raw.as_ptr()) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
}

struct ProtectedValue {
    handle: ProtectedHandle<sys::kunlun_jsc_value, ContextInner>,
}

impl ProtectedValue {
    fn new(
        context: Rc<ContextInner>,
        raw: ValueRef,
        operation: &'static str,
    ) -> Result<Self, JscError> {
        let null_error = JscError::missing_value(operation, "JavaScriptCore returned no value");
        let handle =
            ProtectedHandle::try_new(context, raw, protect_value, unprotect_value, null_error)?;
        Ok(Self { handle })
    }

    fn try_clone(&self) -> Result<Self, JscError> {
        Ok(Self {
            handle: self.handle.try_clone(protect_value)?,
        })
    }

    fn as_value(&self) -> ValueRef {
        self.handle.as_ptr()
    }

    fn as_object(&self) -> ObjectRef {
        self.handle.as_ptr().cast_mut()
    }

    fn context(&self) -> &Rc<ContextInner> {
        self.handle.context()
    }
}

/// A GC-rooted JavaScriptCore value borrowed from a [`JscVm`].
///
/// Its lifetime prevents the public handle from outliving the VM, while the
/// internal protection guard pairs one protect/unprotect for every clone.
///
/// ```compile_fail
/// use kunlun_jsc::RootedValue;
/// fn assert_send<T: Send>() {}
/// assert_send::<RootedValue<'static>>();
/// ```
///
/// ```compile_fail
/// use kunlun_jsc::RootedValue;
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<RootedValue<'static>>();
/// ```
///
/// ```compile_fail
/// use kunlun_jsc::{JscVm, RootedValue};
/// fn outlive_owner(vm: &JscVm) -> RootedValue<'static> {
///     vm.evaluate_rooted("42", "test:///lifetime.js").unwrap()
/// }
/// ```
pub struct RootedValue<'context> {
    protected: ProtectedValue,
    source_url: Option<String>,
    _owner: PhantomData<&'context JscVm>,
}

impl RootedValue<'_> {
    pub fn try_clone(&self) -> Result<Self, JscError> {
        Ok(Self {
            protected: self.protected.try_clone()?,
            source_url: self.source_url.clone(),
            _owner: PhantomData,
        })
    }

    pub fn to_string(&self) -> Result<String, JscError> {
        self.protected.context().value_to_string(
            self.protected.as_value(),
            "rooted_value_to_string",
            self.source_url.as_deref(),
        )
    }
}

impl Clone for RootedValue<'_> {
    fn clone(&self) -> Self {
        self.try_clone()
            .expect("protecting an already rooted JavaScriptCore value must succeed")
    }
}

/// A protected pair of JavaScriptCore Promise resolver functions.
///
/// The Rc keeps the context alive until a scheduled local task settles or
/// drops the Promise. This type is deliberately `!Send + !Sync`.
///
/// ```compile_fail
/// use kunlun_jsc::DeferredPromise;
/// fn assert_send<T: Send>() {}
/// assert_send::<DeferredPromise>();
/// ```
///
/// ```compile_fail
/// use kunlun_jsc::DeferredPromise;
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<DeferredPromise>();
/// ```
pub struct DeferredPromise {
    resolve: ProtectedValue,
    reject: ProtectedValue,
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
            return Err(context.exception_error("deferred_promise_create", None, exception));
        }
        expect_status("deferred_promise_create", status)?;
        if promise.is_null() || resolve.is_null() || reject.is_null() {
            return Err(JscError::missing_value(
                "deferred_promise_create",
                "JavaScriptCore returned an incomplete Promise tuple",
            ));
        }

        let resolve = ProtectedValue::new(
            Rc::clone(&context),
            resolve.cast_const(),
            "deferred_promise_resolve",
        )?;
        // If this second protection fails, `resolve` drops and removes its one
        // protection count before the error is returned.
        let reject = ProtectedValue::new(context, reject.cast_const(), "deferred_promise_reject")?;
        Ok((promise, Self { resolve, reject }))
    }

    fn context(&self) -> &ContextInner {
        self.resolve.context()
    }

    pub fn resolve_undefined(self) -> Result<(), JscError> {
        let mut value = ptr::null();
        // SAFETY: undefined is created in the same live context.
        let status = unsafe {
            sys::kunlun_jsc_value_make_undefined(self.context().as_context(), &mut value)
        };
        expect_status("value_make_undefined", status)?;
        self.settle(&self.resolve, value, "promise_resolve")
    }

    pub fn resolve_string(self, value: &str) -> Result<(), JscError> {
        let value = OwnedJsString::new(value, "promise_resolve")?;
        let mut raw_value = ptr::null();
        // SAFETY: the JS string and resulting value belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_value_make_string(
                self.context().as_context(),
                value.as_ptr(),
                &mut raw_value,
            )
        };
        expect_status("value_make_string", status)?;
        self.settle(&self.resolve, raw_value, "promise_resolve")
    }

    pub fn reject_message(self, message: &str) -> Result<(), JscError> {
        let message = OwnedJsString::new(message, "promise_reject")?;
        let mut value = ptr::null();
        // SAFETY: the JS string and resulting value belong to the same context.
        let status = unsafe {
            sys::kunlun_jsc_value_make_string(
                self.context().as_context(),
                message.as_ptr(),
                &mut value,
            )
        };
        expect_status("value_make_string", status)?;
        self.settle(&self.reject, value, "promise_reject")
    }

    fn settle(
        &self,
        function: &ProtectedValue,
        value: ValueRef,
        operation: &'static str,
    ) -> Result<(), JscError> {
        let arguments = [value];
        let mut result = ptr::null();
        let mut exception = ptr::null();
        // SAFETY: the protected resolver and argument belong to the same live
        // context. JSC invokes the Promise reactions before returning control.
        let status = unsafe {
            sys::kunlun_jsc_object_call_as_function(
                self.context().as_context(),
                function.as_object(),
                ptr::null_mut(),
                u32::try_from(arguments.len()).expect("one Promise settlement argument"),
                arguments.as_ptr(),
                &mut result,
                &mut exception,
            )
        };
        if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !exception.is_null() {
            return Err(self.context().exception_error(operation, None, exception));
        }
        expect_status(operation, status)
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
    let operation =
        match hook
            .context
            .value_to_string(values[0], "host_call_operation_to_string", None)
        {
            Ok(value) => value,
            Err(error) => {
                return callback_error(context, out_exception, &error.to_string());
            }
        };
    let payload = match hook
        .context
        .value_to_string(values[1], "host_call_payload_to_string", None)
    {
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
    let Ok(message) = OwnedJsString::new(message, "callback_error_create") else {
        return;
    };
    let mut error = ptr::null_mut();
    let mut creation_exception = ptr::null();
    // SAFETY: the caller supplied writable exception storage and the string is
    // live for the Error creation call.
    let status = unsafe {
        sys::kunlun_jsc_object_make_error(
            context,
            message.as_ptr(),
            &mut error,
            &mut creation_exception,
        )
    };
    if status == sys::KUNLUN_JSC_STATUS_OK && !error.is_null() {
        // SAFETY: the shim supplied writable callback exception storage.
        unsafe { *out_exception = error.cast_const() };
    } else if status == sys::KUNLUN_JSC_STATUS_JS_EXCEPTION && !creation_exception.is_null() {
        // SAFETY: the shim supplied writable callback exception storage.
        unsafe { *out_exception = creation_exception };
    }
}

struct OwnedJsString {
    handle: OwnedHandle<sys::kunlun_jsc_string>,
}

impl OwnedJsString {
    fn new(value: &str, operation: &'static str) -> Result<Self, JscError> {
        if value.as_bytes().contains(&0) {
            return Err(JscError::invalid_input(
                operation,
                "string contains an interior NUL byte",
            ));
        }
        let length = u64::try_from(value.len()).map_err(|_| {
            JscError::invalid_input(operation, "string length does not fit the C ABI")
        })?;
        let mut raw = ptr::null_mut();
        // SAFETY: the byte slice is live for the call and `raw` is writable
        // output storage. The shim copies the UTF-8 bytes.
        let status =
            unsafe { sys::kunlun_jsc_string_create_utf8(value.as_ptr(), length, &mut raw) };
        expect_status(operation, status)?;
        // SAFETY: success transfers one owned string handle.
        let handle = unsafe { OwnedHandle::from_raw(raw, release_string) }.ok_or_else(|| {
            JscError::missing_value(operation, "JavaScriptCore returned no owned string")
        })?;
        Ok(Self { handle })
    }

    fn as_ptr(&self) -> *mut sys::kunlun_jsc_string {
        self.handle.as_ptr()
    }

    fn to_utf8(&self, operation: &'static str) -> Result<String, JscError> {
        let mut capacity = 0_u64;
        // SAFETY: the handle is a live owned JS string.
        let status =
            unsafe { sys::kunlun_jsc_string_get_max_utf8_size(self.as_ptr(), &mut capacity) };
        expect_status(operation, status)?;
        if capacity == 0 {
            return Err(JscError::missing_value(
                operation,
                "JavaScriptCore reported a zero-sized UTF-8 buffer",
            ));
        }

        let capacity = usize::try_from(capacity).map_err(|_| {
            JscError::invalid_input(operation, "UTF-8 buffer size does not fit usize")
        })?;
        let mut buffer = vec![0_u8; capacity];
        let mut written = 0_u64;
        // SAFETY: the buffer has the capacity reported by JSC and is writable.
        let status = unsafe {
            sys::kunlun_jsc_string_write_utf8(
                self.as_ptr(),
                buffer.as_mut_ptr(),
                u64::try_from(capacity).map_err(|_| {
                    JscError::invalid_input(operation, "UTF-8 buffer size does not fit the C ABI")
                })?,
                &mut written,
            )
        };
        expect_status(operation, status)?;
        let written = usize::try_from(written).map_err(|_| {
            JscError::invalid_input(operation, "UTF-8 output length does not fit usize")
        })?;
        if written == 0 || written > buffer.len() || buffer[written - 1] != 0 {
            return Err(JscError::missing_value(
                operation,
                "JavaScriptCore returned malformed UTF-8 output",
            ));
        }

        String::from_utf8(buffer[..written - 1].to_vec()).map_err(|_| {
            JscError::invalid_input(operation, "JavaScriptCore returned invalid UTF-8")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_javascript() {
        let vm = JscVm::new("kunlun-test").expect("create VM");
        assert_eq!(vm.evaluate("21 * 2", "test:///eval.js").unwrap(), "42");
    }

    #[test]
    fn explicit_group_outlives_its_public_handle_while_contexts_exist() {
        let group = ContextGroup::new().expect("create context group");
        let first = group.create_vm("kunlun-group-first").expect("first VM");
        let second = group.create_vm("kunlun-group-second").expect("second VM");
        drop(group);

        assert_eq!(first.evaluate("6 * 7", "test:///first.js").unwrap(), "42");
        assert_eq!(second.evaluate("7 * 8", "test:///second.js").unwrap(), "56");
    }

    #[test]
    fn rooted_values_and_clones_remain_live_until_each_guard_drops() {
        let vm = JscVm::new("kunlun-root-test").expect("create VM");
        let rooted = vm
            .evaluate_rooted("({ answer: 42 })", "test:///rooted.js")
            .expect("evaluate rooted value");
        let clone = rooted.clone();

        assert_eq!(rooted.to_string().unwrap(), "[object Object]");
        drop(rooted);
        assert_eq!(clone.to_string().unwrap(), "[object Object]");
    }

    #[test]
    fn invalid_source_errors_keep_the_operation_and_source_url() {
        let vm = JscVm::new("kunlun-invalid-source-test").expect("create VM");
        let error = vm
            .evaluate("'interior\0nul'", "test:///invalid-source.js")
            .unwrap_err();

        assert_eq!(error.operation(), "evaluate");
        assert_eq!(error.kind(), crate::JscErrorKind::InvalidInput);
        assert_eq!(error.source_url(), Some("test:///invalid-source.js"));
        assert_eq!(error.status(), None);
        assert!(error.detail().unwrap().contains("interior NUL"));
    }

    #[test]
    fn returns_javascript_exceptions() {
        let vm = JscVm::new("kunlun-test").expect("create VM");
        let error = vm
            .evaluate("throw new Error('boom')", "test:///exception.js")
            .unwrap_err();
        assert_eq!(error.operation(), "evaluate");
        assert_eq!(error.kind(), crate::JscErrorKind::JavaScriptException);
        assert_eq!(error.status(), Some(crate::JscStatus::JavaScriptException));
        assert_eq!(error.source_url(), Some("test:///exception.js"));
        assert!(error.exception_text().unwrap().contains("boom"));
    }

    #[test]
    fn returns_javascript_exceptions_from_string_conversion() {
        let vm = JscVm::new("kunlun-test").expect("create VM");
        let error = vm
            .evaluate(
                "({ toString() { throw new Error('conversion boom'); } })",
                "test:///string-conversion-exception.js",
            )
            .unwrap_err();
        assert_eq!(error.operation(), "rooted_value_to_string");
        assert_eq!(
            error.source_url(),
            Some("test:///string-conversion-exception.js")
        );
        assert!(error.exception_text().unwrap().contains("conversion boom"));
    }

    #[test]
    fn falls_back_when_stringifying_a_conversion_exception_fails() {
        let vm = JscVm::new("kunlun-test").expect("create VM");
        let error = vm
            .evaluate(
                "({ toString() { throw this; } })",
                "test:///recursive-string-conversion-exception.js",
            )
            .unwrap_err();
        assert_eq!(error.kind(), crate::JscErrorKind::JavaScriptException);
        assert_eq!(
            error.exception_text(),
            Some(EXCEPTION_STRINGIFICATION_FALLBACK)
        );
        assert_eq!(
            error.source_url(),
            Some("test:///recursive-string-conversion-exception.js")
        );
    }

    #[test]
    fn toggles_web_inspector_visibility() {
        let vm = JscVm::new("kunlun-inspector-test").expect("create VM");
        assert!(!vm.is_inspectable().unwrap());
        vm.set_inspectable(true).unwrap();
        assert!(vm.is_inspectable().unwrap());
        vm.set_inspectable(false).unwrap();
        assert!(!vm.is_inspectable().unwrap());
    }

    #[test]
    fn contains_rust_panics_in_host_callbacks() {
        let vm = JscVm::new("kunlun-panic-test").expect("create VM");
        vm.install_sleep_scheduler(|_, _| panic!("test scheduler panic"))
            .expect("install sleep callback");

        let caught = vm
            .evaluate(
                "try { sleep(1); } catch (error) { [error instanceof Error, error.message, typeof error.stack === 'string' && error.stack.length > 0].join('|'); }",
                "test:///callback-panic.js",
            )
            .expect("callback panic can be caught as a JavaScript Error");
        assert_eq!(caught, "true|Kunlun timer scheduler panicked|true");
    }

    #[test]
    fn callback_registration_accepts_and_releases_non_send_state() {
        use std::cell::Cell;

        let calls = Rc::new(Cell::new(0));
        {
            let vm = JscVm::new("kunlun-local-callback-test").expect("create VM");
            let callback_calls = Rc::clone(&calls);
            vm.install_sleep_scheduler(move |_, promise| {
                callback_calls.set(callback_calls.get() + 1);
                promise.resolve_undefined().expect("resolve Promise");
            })
            .expect("install isolate-local callback");
            assert_eq!(Rc::strong_count(&calls), 2);
        }
        assert_eq!(Rc::strong_count(&calls), 1);
    }
}
