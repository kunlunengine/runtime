//! Safe, thread-affine JavaScriptCore primitives for Kunlun Runtime.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[cfg(target_os = "macos")]
pub use macos::{DeferredPromise, JscVm};
#[cfg(not(target_os = "macos"))]
pub use unsupported::{DeferredPromise, JscVm};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCall {
    pub operation: String,
    pub payload: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendInfo {
    pub name: &'static str,
    pub distribution: &'static str,
    pub hermetic: bool,
    pub supports_inspection: bool,
    pub supports_deferred_promises: bool,
    pub supports_native_modules: bool,
    pub supports_explicit_microtask_checkpoint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JscError {
    UnsupportedPlatform(&'static str),
    ContextCreation,
    InvalidString,
    Exception(String),
    ValueConversion,
    PromiseCreation,
    HostFunction(String),
}

impl Display for JscError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(message) => formatter.write_str(message),
            Self::ContextCreation => formatter.write_str("JavaScriptCore did not create a context"),
            Self::InvalidString => {
                formatter.write_str("JavaScript source contains an interior NUL byte")
            }
            Self::Exception(message) => write!(formatter, "JavaScript exception: {message}"),
            Self::ValueConversion => {
                formatter.write_str("JavaScriptCore value could not be converted to UTF-8")
            }
            Self::PromiseCreation => formatter.write_str("JavaScriptCore did not create a Promise"),
            Self::HostFunction(message) => write!(formatter, "host function error: {message}"),
        }
    }
}

impl Error for JscError {}
