//! Native module records and isolate-local resolve/fetch callbacks.
use super::*;
use std::rc::Weak;

/// Supplies canonical URL identities and JavaScript sources to JSC. Both
/// static and dynamic imports use these same synchronous callbacks. No engine
/// values cross this interface; implementations can capture isolate-local state.
pub trait ModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, String>;
    fn fetch(&self, canonical_url: &str) -> Result<String, String>;
    /// Adds source-map locations while retaining the original engine stack.
    fn map_error(&self, description: &str) -> String {
        description.to_owned()
    }
}

struct ModuleHook {
    context: Weak<ContextInner>,
    loader: Box<dyn ModuleLoader>,
}

thread_local! {
    static MODULE_HOOKS: RefCell<HashMap<usize, Rc<ModuleHook>>> = RefCell::new(HashMap::new());
}

pub(super) fn revoke(context: ContextRef) {
    // SAFETY: called on the isolate thread before its context or callback state
    // is released. Native revocation does not invoke user code.
    let status = unsafe { sys::kunlun_jsc_modules_revoke(context) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
    MODULE_HOOKS.with(|hooks| {
        hooks.borrow_mut().remove(&(context as usize));
    });
}

impl JscVm {
    fn module_error(&self, operation: &'static str, url: &str, exception: ValueRef) -> JscError {
        // Root before stringification: a user-defined toString can reenter JS
        // and trigger collection just like a stack getter can.
        let root =
            ProtectedValue::new(Rc::clone(&self.context), exception, "module_exception_root").ok();
        let original = self
            .context
            .exception_error(operation, Some(url), exception);
        let mut description = original.exception_text().unwrap_or_default().to_owned();
        if let (Some(root), Ok(name)) = (
            root.as_ref(),
            OwnedJsString::new("stack", "module_exception_stack"),
        ) {
            let mut stack = ptr::null();
            let mut getter_error = ptr::null();
            // SAFETY: the exception is rooted across a possibly reentrant or
            // throwing stack getter. Every returned value belongs to this VM.
            let status = unsafe {
                sys::kunlun_jsc_value_get_property(
                    self.context.as_context(),
                    root.as_value(),
                    name.as_ptr(),
                    &mut stack,
                    &mut getter_error,
                )
            };
            if status == sys::KUNLUN_JSC_STATUS_OK && !stack.is_null() {
                if let Ok(stack) =
                    self.context
                        .value_to_string(stack, "module_exception_stack", None)
                {
                    if stack != "undefined" && !stack.is_empty() {
                        description.push('\n');
                        description.push_str(&stack);
                    }
                }
            }
        }
        let hook = MODULE_HOOKS.with(|hooks| {
            hooks
                .borrow()
                .get(&(self.context.as_context() as usize))
                .cloned()
        });
        if let Some(hook) = hook {
            if let Ok(mapped) = catch_callback_panic(|| hook.loader.map_error(&description)) {
                description = mapped;
            }
        }
        JscError::exception(operation, Some(url), description)
    }

    /// Installs one resolver/fetcher for this VM's lifetime. Replacing it would
    /// invalidate JSC's cache authority and is rejected. Only bundled JSC has
    /// native module hooks; system JSC reports `JscStatus::Unsupported`.
    pub fn install_module_loader(
        &mut self,
        loader: impl ModuleLoader + 'static,
    ) -> Result<(), JscError> {
        let context = self.context.as_context();
        let hook = Rc::new(ModuleHook {
            context: Rc::downgrade(&self.context),
            loader: Box::new(loader),
        });
        let inserted = MODULE_HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            if hooks.contains_key(&(context as usize)) {
                return false;
            }
            hooks.insert(context as usize, hook);
            true
        });
        if !inserted {
            return Err(JscError::invalid_input(
                "modules_install",
                "a module loader is already installed",
            ));
        }
        // SAFETY: the trampoline has static code lifetime; the registry owns
        // Rust state before native registration, and no borrow spans this call.
        let status = unsafe { sys::kunlun_jsc_modules_install(context, Some(module_callback)) };
        if let Err(error) = expect_status("modules_install", status) {
            revoke(context);
            return Err(error);
        }
        Ok(())
    }

    /// Fetches and parses a native ESM graph under the installed loader. The
    /// entry is resolved with a null referrer before touching the engine cache.
    pub fn load_module(&self, specifier: &str) -> Result<ModuleRecord<'_>, JscError> {
        let hook = MODULE_HOOKS
            .with(|hooks| {
                hooks
                    .borrow()
                    .get(&(self.context.as_context() as usize))
                    .cloned()
            })
            .ok_or_else(|| {
                JscError::invalid_input("module_load", "no module loader is installed")
            })?;
        let url = catch_callback_panic(|| hook.loader.resolve(specifier, None))
            .map_err(|_| JscError::host_function("module_resolve", "module resolver panicked"))?
            .map_err(|error| JscError::host_function("module_resolve", error))?;
        let key = OwnedJsString::new(&url, "module_load")?;
        let mut raw = ptr::null_mut();
        let mut exception = ptr::null();
        // SAFETY: the context and canonical URL are live; successful creation
        // transfers one rooted native handle tied to this VM borrow.
        let status = unsafe {
            sys::kunlun_jsc_module_load(
                self.context.as_context(),
                key.as_ptr(),
                &mut raw,
                &mut exception,
            )
        };
        if !exception.is_null() {
            return Err(self.module_error("module_load", &url, exception));
        }
        expect_status("module_load", status)?;
        // SAFETY: a successful native call transfers exactly one module owner.
        let handle = unsafe { OwnedHandle::from_raw(raw, release_module) }
            .ok_or_else(|| JscError::missing_value("module_load", "JSC returned no module"))?;
        Ok(ModuleRecord {
            handle,
            vm: self,
            url,
        })
    }
}

unsafe fn release_module(raw: NonNull<sys::kunlun_jsc_module>) {
    // SAFETY: OwnedHandle releases this rooted module exactly once while its
    // borrowed VM is still live, on the isolate thread.
    let status = unsafe { sys::kunlun_jsc_module_release(raw.as_ptr()) };
    debug_assert_eq!(status, sys::KUNLUN_JSC_STATUS_OK);
}

/// Owns a root for a native module phase and borrows its VM. JSC owns graph and
/// cache semantics, including live bindings and cycles. A dropped handle does
/// not remove an evaluated module from that VM's cache.
///
/// ```compile_fail
/// use kunlun_jsc::ModuleRecord;
/// fn send<T: Send>() {}
/// send::<ModuleRecord<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::ModuleRecord;
/// fn sync<T: Sync>() {}
/// sync::<ModuleRecord<'static>>();
/// ```
/// ```compile_fail
/// use kunlun_jsc::JscVm;
/// let module = {
///     let vm = JscVm::new("module-lifetime").unwrap();
///     vm.load_module("file:///entry.mjs").unwrap()
/// };
/// module.poll().unwrap();
/// ```
pub struct ModuleRecord<'vm> {
    handle: OwnedHandle<sys::kunlun_jsc_module>,
    vm: &'vm JscVm,
    url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleState {
    Pending,
    Fulfilled,
}

impl ModuleRecord<'_> {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Links/evaluates a successfully loaded graph. Pending TLA is observed
    /// through `poll`; this function never blocks waiting for host work.
    pub fn evaluate(&mut self) -> Result<(), JscError> {
        let mut exception = ptr::null();
        // SAFETY: the record owns its native root and borrows a live VM.
        let status =
            unsafe { sys::kunlun_jsc_module_evaluate(self.handle.as_ptr(), &mut exception) };
        if !exception.is_null() {
            return Err(self
                .vm
                .module_error("module_evaluate", &self.url, exception));
        }
        expect_status("module_evaluate", status)
    }

    /// Observes the native Promise state without invoking replaceable JS
    /// properties. Rejections are returned as structured JavaScript errors.
    pub fn poll(&self) -> Result<ModuleState, JscError> {
        let mut state = 0;
        let mut exception = ptr::null();
        // SAFETY: the native record and its retained Promise belong to this VM.
        let status = unsafe {
            sys::kunlun_jsc_module_poll(self.handle.as_ptr(), &mut state, &mut exception)
        };
        expect_status("module_poll", status)?;
        match state {
            0 => Ok(ModuleState::Pending),
            1 => Ok(ModuleState::Fulfilled),
            2 if !exception.is_null() => {
                Err(self.vm.module_error("module_poll", &self.url, exception))
            }
            _ => Err(JscError::missing_value(
                "module_poll",
                "JSC returned an invalid module settlement",
            )),
        }
    }
}

unsafe extern "C" fn module_callback(
    context: ContextRef,
    operation: u32,
    key: ValueRef,
    referrer: ValueRef,
    out_result: *mut ValueRef,
    out_exception: *mut ValueRef,
) -> sys::kunlun_jsc_status {
    match catch_callback_panic(|| {
        let hook = MODULE_HOOKS.with(|hooks| hooks.borrow().get(&(context as usize)).cloned());
        let Some(hook) = hook else {
            return callback_error(context, out_exception, "module loader was revoked");
        };
        let Some(owner) = hook.context.upgrade() else {
            return callback_error(context, out_exception, "module context was released");
        };
        let result = (|| -> Result<String, String> {
            let key = owner
                .value_to_string(key, "module_callback_key", None)
                .map_err(|e| e.to_string())?;
            let referrer = if referrer.is_null() {
                None
            } else {
                Some(
                    owner
                        .value_to_string(referrer, "module_callback_referrer", None)
                        .map_err(|e| e.to_string())?,
                )
            };
            match operation {
                0 => hook.loader.resolve(&key, referrer.as_deref()),
                1 => hook.loader.fetch(&key),
                _ => Err("invalid module callback operation".to_owned()),
            }
        })();
        match result {
            Ok(result) => match OwnedJsString::new(&result, "module_callback_result") {
                Ok(string) => {
                    // SAFETY: the native callback supplies writable output;
                    // string data is copied into a context-owned JS value.
                    unsafe {
                        sys::kunlun_jsc_value_make_string(context, string.as_ptr(), out_result)
                    }
                }
                Err(error) => callback_error(context, out_exception, &error.to_string()),
            },
            Err(error) => callback_error(context, out_exception, &error),
        }
    }) {
        Ok(status) => status,
        Err(_) => callback_error(context, out_exception, "Kunlun module callback panicked"),
    }
}
