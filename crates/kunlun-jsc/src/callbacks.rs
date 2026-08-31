//! Revocable, isolate-owned callbacks. GC never owns a Rust closure.
use super::*;
use std::ffi::c_void;
use std::rc::Weak;

/// A plain-data result from a host function. No engine pointer can escape.
#[derive(Debug, Clone, PartialEq)]
pub enum CallbackReturn {
    Undefined,
    Boolean(bool),
    Number(f64),
    String(String),
}

/// A callback argument borrowed only for this invocation, on this thread.
///
/// ```compile_fail
/// use kunlun_jsc::CallbackValue;
/// fn send<T: Send>() {}
/// send::<CallbackValue<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::CallbackValue;
/// fn sync<T: Sync>() {}
/// sync::<CallbackValue<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::{JscVm, CallbackReturn};
/// use std::cell::RefCell;
/// let vm = JscVm::new("arguments").unwrap();
/// let saved = RefCell::new(None);
/// vm.host_function("capture", move |args| {
///     *saved.borrow_mut() = Some(&args[0]);
///     Ok(CallbackReturn::Undefined)
/// }).unwrap();
/// ```
pub struct CallbackValue<'call> {
    context: &'call ContextInner,
    raw: ValueRef,
}

impl CallbackValue<'_> {
    pub fn to_string(&self) -> Result<String, JscError> {
        self.context
            .value_to_string(self.raw, "callback_value_to_string", None)
    }

    pub fn to_number(&self) -> Result<f64, JscError> {
        let mut number = 0.0;
        let mut exception = ptr::null();
        // SAFETY: JSC retains the argument for this synchronous invocation.
        let status = unsafe {
            sys::kunlun_jsc_value_to_number(
                self.context.as_context(),
                self.raw,
                &mut number,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self
                .context
                .exception_error("callback_value_to_number", None, exception));
        }
        expect_status("callback_value_to_number", status)?;
        Ok(number)
    }
}

type Callback = dyn for<'call> Fn(&[CallbackValue<'call>]) -> Result<CallbackReturn, String>;
struct CallbackState {
    context: Weak<ContextInner>,
    callback: Box<Callback>,
}

/// Owns a rooted host function and its isolate-local closure. Keep this handle
/// alive while JS may call the function. Drop revokes all JS aliases before
/// releasing the closure; later calls throw. Captures may be `!Send + !Sync`.
/// An in-flight reentrant invocation retains the state until it returns.
///
/// ```compile_fail
/// use kunlun_jsc::HostFunction;
/// fn send<T: Send>() {}
/// send::<HostFunction<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::HostFunction;
/// fn sync<T: Sync>() {}
/// sync::<HostFunction<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::{JscVm, CallbackReturn};
/// let function = {
///     let vm = JscVm::new("temporary").unwrap();
///     vm.host_function("f", |_| Ok(CallbackReturn::Undefined)).unwrap()
/// };
/// function.set_global("f").unwrap();
/// ```
pub struct HostFunction<'context> {
    value: RootedValue<'context>,
    _state: Rc<CallbackState>,
}

impl HostFunction<'_> {
    pub fn set_global(&self, name: &str) -> Result<(), JscError> {
        self.value.set_global(name)
    }
}

impl Drop for HostFunction<'_> {
    fn drop(&mut self) {
        // SAFETY: the root keeps the function/context live, and this !Send
        // handle can only be dropped on the creating isolate thread.
        let status = unsafe {
            sys::kunlun_jsc_object_revoke_function(
                self.value.protected.context().as_context(),
                self.value.protected.as_object(),
            )
        };
        // A failed revocation must never leave a dangling user_data pointer.
        if status != sys::KUNLUN_JSC_STATUS_OK {
            std::process::abort();
        }
    }
}

impl JscVm {
    pub fn host_function<F>(&self, name: &str, callback: F) -> Result<HostFunction<'_>, JscError>
    where
        F: for<'call> Fn(&[CallbackValue<'call>]) -> Result<CallbackReturn, String> + 'static,
    {
        let name = OwnedJsString::new(name, "host_function")?;
        let state = Rc::new(CallbackState {
            context: Rc::downgrade(&self.context),
            callback: Box::new(callback),
        });
        let mut function = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: Rc allocation stays live until revocation; native finalization
        // never accesses or drops it. Callback code has static lifetime.
        let status = unsafe {
            sys::kunlun_jsc_object_make_function_with_data(
                self.context.as_context(),
                name.as_ptr(),
                Some(callback_bridge),
                Rc::as_ptr(&state).cast_mut().cast(),
                &mut function,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self
                .context
                .exception_error("host_function", None, exception));
        }
        expect_status("host_function", status)?;
        let protected =
            match ProtectedValue::new(Rc::clone(&self.context), function, "host_function") {
                Ok(value) => value,
                Err(error) => {
                    // SAFETY: successful creation yielded a still-live function.
                    // Revoke before dropping the Rc, even when rooting fails.
                    let status = unsafe {
                        sys::kunlun_jsc_object_revoke_function(self.context.as_context(), function)
                    };
                    if status != sys::KUNLUN_JSC_STATUS_OK {
                        std::process::abort();
                    }
                    return Err(error);
                }
            };
        Ok(HostFunction {
            value: RootedValue {
                protected,
                source_url: None,
                _owner: PhantomData,
            },
            _state: state,
        })
    }
}

unsafe extern "C" fn callback_bridge(
    user_data: *mut c_void,
    context: ContextRef,
    argument_count: u32,
    arguments: *const ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    let result = catch_callback_panic(|| {
        // SAFETY: native dispatch enforces the owning thread and revocation.
        // Retain before invoking user code, which may drop its own handle.
        let state = unsafe {
            let raw = user_data.cast::<CallbackState>();
            Rc::increment_strong_count(raw);
            Rc::from_raw(raw)
        };
        let owner = state
            .context
            .upgrade()
            .ok_or("Kunlun callback context is unavailable")?;
        if owner.as_context() != context {
            return Err("Kunlun callback invoked from a different context".to_owned());
        }
        let raw_arguments = if argument_count == 0 {
            &[]
        } else {
            // SAFETY: the shim supplies exactly argument_count live arguments.
            unsafe { std::slice::from_raw_parts(arguments, argument_count as usize) }
        };
        let values: Vec<_> = raw_arguments
            .iter()
            .map(|&raw| CallbackValue {
                context: &owner,
                raw,
            })
            .collect();
        (state.callback)(&values)
    });
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => return callback_error(context, out_exception, &message),
        Err(()) => {
            return callback_error(context, out_exception, "Kunlun host callback panicked");
        }
    };
    // SAFETY: output storage and context are provided by the synchronous shim;
    // all returned values are borrowed from that context.
    unsafe {
        match value {
            CallbackReturn::Undefined => sys::kunlun_jsc_value_make_undefined(context, out_result),
            CallbackReturn::Boolean(value) => {
                sys::kunlun_jsc_value_make_boolean(context, u8::from(value), out_result)
            }
            CallbackReturn::Number(value) => {
                sys::kunlun_jsc_value_make_number(context, value, out_result)
            }
            CallbackReturn::String(value) => match OwnedJsString::new(&value, "callback_return") {
                Ok(string) => {
                    sys::kunlun_jsc_value_make_string(context, string.as_ptr(), out_result)
                }
                Err(_) => callback_error(context, out_exception, "Invalid host callback string"),
            },
        }
    }
}
