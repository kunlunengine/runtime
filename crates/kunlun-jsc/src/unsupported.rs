use crate::{BackendInfo, HostCall, JscError};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

const UNAVAILABLE: &str = "the bootstrap currently links the macOS JavaScriptCore framework; use macOS or wait for the pinned WebKit distribution";

/// A thread-affine JavaScriptCore context group.
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
    _thread_affinity: PhantomData<Rc<()>>,
}

/// A thread-affine JavaScriptCore global context.
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
    _thread_affinity: PhantomData<Rc<()>>,
}

/// A GC-rooted value tied to its [`JscVm`].
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
    _owner: PhantomData<&'context JscVm>,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl RootedValue<'_> {
    pub fn try_clone(&self) -> Result<Self, JscError> {
        Err(JscError::unsupported("value_protect", UNAVAILABLE))
    }

    pub fn to_string(&self) -> Result<String, JscError> {
        Err(JscError::unsupported("rooted_value_to_string", UNAVAILABLE))
    }
}

impl Clone for RootedValue<'_> {
    fn clone(&self) -> Self {
        Self {
            _owner: PhantomData,
            _thread_affinity: PhantomData,
        }
    }
}

/// A protected, thread-affine Promise settlement handle.
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
    _thread_affinity: PhantomData<Rc<()>>,
}

impl DeferredPromise {
    pub fn resolve_undefined(self) -> Result<(), JscError> {
        Err(JscError::unsupported("promise_resolve", UNAVAILABLE))
    }

    pub fn resolve_string(self, _value: &str) -> Result<(), JscError> {
        Err(JscError::unsupported("promise_resolve", UNAVAILABLE))
    }

    pub fn reject_message(self, _message: &str) -> Result<(), JscError> {
        Err(JscError::unsupported("promise_reject", UNAVAILABLE))
    }
}

impl ContextGroup {
    pub fn new() -> Result<Self, JscError> {
        Err(JscError::unsupported("context_group_create", UNAVAILABLE))
    }

    pub fn create_vm(&self, _name: &str) -> Result<JscVm, JscError> {
        Err(JscError::unsupported(
            "context_create_in_group",
            UNAVAILABLE,
        ))
    }
}

impl JscVm {
    pub const fn backend_info() -> BackendInfo {
        BackendInfo {
            name: "JavaScriptCore",
            distribution: "unavailable in the bootstrap build",
            hermetic: false,
            supports_inspection: false,
            supports_deferred_promises: false,
            supports_native_modules: false,
            supports_explicit_microtask_checkpoint: false,
        }
    }

    pub fn new(_name: &str) -> Result<Self, JscError> {
        Err(JscError::unsupported("context_create", UNAVAILABLE))
    }

    pub fn set_name(&self, _name: &str) -> Result<(), JscError> {
        Err(JscError::unsupported("context_set_name", UNAVAILABLE))
    }

    pub fn set_inspectable(&self, _inspectable: bool) -> Result<(), JscError> {
        Err(JscError::unsupported(
            "context_set_inspectable",
            UNAVAILABLE,
        ))
    }

    pub fn is_inspectable(&self) -> Result<bool, JscError> {
        Err(JscError::unsupported("context_is_inspectable", UNAVAILABLE))
    }

    /// Registers only isolate-local callback state.
    ///
    /// ```compile_fail
    /// use kunlun_jsc::DeferredPromise;
    /// use std::time::Duration;
    /// type Callback = dyn Fn(Duration, DeferredPromise);
    /// fn assert_send_sync<T: ?Sized + Send + Sync>() {}
    /// assert_send_sync::<Callback>();
    /// ```
    pub fn install_sleep_scheduler<F>(&self, _schedule: F) -> Result<(), JscError>
    where
        F: Fn(Duration, DeferredPromise) + 'static,
    {
        Err(JscError::unsupported(
            "install_sleep_scheduler",
            UNAVAILABLE,
        ))
    }

    pub fn install_host_call_scheduler<F>(&self, _schedule: F) -> Result<(), JscError>
    where
        F: Fn(HostCall, DeferredPromise) + 'static,
    {
        Err(JscError::unsupported(
            "install_host_call_scheduler",
            UNAVAILABLE,
        ))
    }

    pub fn evaluate(&self, _source: &str, source_url: &str) -> Result<String, JscError> {
        Err(JscError::unsupported("evaluate", UNAVAILABLE).with_source_url(source_url))
    }

    pub fn evaluate_rooted<'context>(
        &'context self,
        _source: &str,
        source_url: &str,
    ) -> Result<RootedValue<'context>, JscError> {
        Err(JscError::unsupported("evaluate", UNAVAILABLE).with_source_url(source_url))
    }
}
