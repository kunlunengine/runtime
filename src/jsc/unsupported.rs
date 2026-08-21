use super::{BackendInfo, JscError};
use std::marker::PhantomData;
use std::rc::Rc;

pub struct JscVm {
    _thread_affinity: PhantomData<Rc<()>>,
}

impl JscVm {
    pub const fn backend_info() -> BackendInfo {
        BackendInfo {
            name: "JavaScriptCore",
            distribution: "unavailable in the bootstrap build",
            hermetic: false,
            supports_inspection: false,
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

    pub fn evaluate(&mut self, _source: &str, _source_url: &str) -> Result<String, JscError> {
        Err(JscError::UnsupportedPlatform(
            "JavaScriptCore is unavailable",
        ))
    }
}
