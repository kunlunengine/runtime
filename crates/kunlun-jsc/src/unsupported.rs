use crate::{BackendInfo, HostCall, JscError};
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

pub struct JscVm {
    _thread_affinity: PhantomData<Rc<()>>,
}

pub struct DeferredPromise {
    _thread_affinity: PhantomData<Rc<()>>,
}

impl DeferredPromise {
    pub fn resolve_undefined(self) -> Result<(), JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }

    pub fn resolve_string(self, _value: &str) -> Result<(), JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }

    pub fn reject_message(self, _message: &str) -> Result<(), JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
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
        Err(JscError::UnsupportedPlatform(
            "the bootstrap currently links the macOS JavaScriptCore framework; use macOS or wait for the pinned WebKit distribution",
        ))
    }

    pub fn set_name(&self, _name: &str) -> Result<(), JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }

    pub fn set_inspectable(&self, _inspectable: bool) {}

    pub fn is_inspectable(&self) -> bool {
        false
    }

    pub fn install_sleep_scheduler<F>(&self, _schedule: F) -> Result<(), JscError>
    where
        F: Fn(Duration, DeferredPromise) + 'static,
    {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }

    pub fn install_host_call_scheduler<F>(&self, _schedule: F) -> Result<(), JscError>
    where
        F: Fn(HostCall, DeferredPromise) + 'static,
    {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }

    pub fn evaluate(&mut self, _source: &str, _source_url: &str) -> Result<String, JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }
}
