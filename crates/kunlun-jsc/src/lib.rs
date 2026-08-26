//! Safe, thread-affine JavaScriptCore primitives for Kunlun Runtime.

#[cfg(target_os = "macos")]
mod macos;
mod ownership;
#[cfg(not(target_os = "macos"))]
mod unsupported;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[cfg(target_os = "macos")]
pub use macos::{ContextGroup, DeferredPromise, JscVm, RootedValue};
#[cfg(not(target_os = "macos"))]
pub use unsupported::{ContextGroup, DeferredPromise, JscVm, RootedValue};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JscErrorKind {
    UnsupportedPlatform,
    InvalidInput,
    JavaScriptException,
    MissingValue,
    HostFunction,
    NativeFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JscStatus {
    Ok,
    InvalidArgument,
    OutOfMemory,
    JavaScriptException,
    BufferTooSmall,
    IntegerOverflow,
    CallbackError,
    CppException,
    Unknown(u32),
}

impl JscStatus {
    pub const fn from_raw(status: u32) -> Self {
        match status {
            0 => Self::Ok,
            1 => Self::InvalidArgument,
            2 => Self::OutOfMemory,
            3 => Self::JavaScriptException,
            4 => Self::BufferTooSmall,
            5 => Self::IntegerOverflow,
            6 => Self::CallbackError,
            7 => Self::CppException,
            status => Self::Unknown(status),
        }
    }

    pub const fn as_raw(self) -> u32 {
        match self {
            Self::Ok => 0,
            Self::InvalidArgument => 1,
            Self::OutOfMemory => 2,
            Self::JavaScriptException => 3,
            Self::BufferTooSmall => 4,
            Self::IntegerOverflow => 5,
            Self::CallbackError => 6,
            Self::CppException => 7,
            Self::Unknown(status) => status,
        }
    }
}

impl Display for JscStatus {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ok => "ok",
            Self::InvalidArgument => "invalid argument",
            Self::OutOfMemory => "out of memory",
            Self::JavaScriptException => "JavaScript exception",
            Self::BufferTooSmall => "buffer too small",
            Self::IntegerOverflow => "integer overflow",
            Self::CallbackError => "callback error",
            Self::CppException => "C++ exception",
            Self::Unknown(status) => return write!(formatter, "unknown status {status}"),
        };
        formatter.write_str(name)
    }
}

/// A structured JavaScriptCore failure with no exposed native pointers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JscError {
    operation: &'static str,
    kind: JscErrorKind,
    status: Option<JscStatus>,
    source_url: Option<String>,
    exception_text: Option<String>,
    detail: Option<String>,
}

impl JscError {
    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub const fn kind(&self) -> JscErrorKind {
        self.kind
    }

    pub const fn status(&self) -> Option<JscStatus> {
        self.status
    }

    pub fn source_url(&self) -> Option<&str> {
        self.source_url.as_deref()
    }

    pub fn exception_text(&self) -> Option<&str> {
        self.exception_text.as_deref()
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn unsupported(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::new(JscErrorKind::UnsupportedPlatform, operation).with_detail(detail)
    }

    /// Constructs an exception captured by a higher-level JavaScript host
    /// operation while retaining its logical operation and source URL.
    pub fn javascript_exception(
        operation: &'static str,
        source_url: Option<&str>,
        exception_text: impl Into<String>,
    ) -> Self {
        Self::exception(operation, source_url, exception_text)
    }

    pub(crate) fn invalid_input(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::new(JscErrorKind::InvalidInput, operation).with_detail(detail)
    }

    pub(crate) fn missing_value(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::new(JscErrorKind::MissingValue, operation).with_detail(detail)
    }

    pub(crate) fn host_function(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::new(JscErrorKind::HostFunction, operation).with_detail(detail)
    }

    pub(crate) fn native(operation: &'static str, status: u32) -> Self {
        Self::new(JscErrorKind::NativeFailure, operation).with_status(JscStatus::from_raw(status))
    }

    pub(crate) fn exception(
        operation: &'static str,
        source_url: Option<&str>,
        exception_text: impl Into<String>,
    ) -> Self {
        let mut error = Self::new(JscErrorKind::JavaScriptException, operation)
            .with_status(JscStatus::JavaScriptException);
        error.source_url = source_url.map(str::to_owned);
        error.exception_text = Some(exception_text.into());
        error
    }

    pub(crate) fn with_source_url(mut self, source_url: &str) -> Self {
        self.source_url = Some(source_url.to_owned());
        self
    }

    const fn new(kind: JscErrorKind, operation: &'static str) -> Self {
        Self {
            operation,
            kind,
            status: None,
            source_url: None,
            exception_text: None,
            detail: None,
        }
    }

    const fn with_status(mut self, status: JscStatus) -> Self {
        self.status = Some(status);
        self
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl Display for JscError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "JavaScriptCore operation {} failed",
            self.operation
        )?;
        if let Some(source_url) = &self.source_url {
            write!(formatter, " for {source_url}")?;
        }
        if let Some(status) = self.status {
            write!(formatter, " ({status})")?;
        }
        if let Some(exception_text) = &self.exception_text {
            write!(formatter, ": {exception_text}")?;
        } else if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for JscError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_values_round_trip_without_losing_unknown_codes() {
        for raw in 0..=7 {
            assert_eq!(JscStatus::from_raw(raw).as_raw(), raw);
        }
        assert_eq!(JscStatus::from_raw(99), JscStatus::Unknown(99));
        assert_eq!(JscStatus::Unknown(99).as_raw(), 99);
    }

    #[test]
    fn structured_exception_preserves_all_diagnostic_fields() {
        let error = JscError::javascript_exception(
            "evaluate_async_body",
            Some("test:///async.js"),
            "Error: boom",
        );

        assert_eq!(error.operation(), "evaluate_async_body");
        assert_eq!(error.kind(), JscErrorKind::JavaScriptException);
        assert_eq!(error.status(), Some(JscStatus::JavaScriptException));
        assert_eq!(error.source_url(), Some("test:///async.js"));
        assert_eq!(error.exception_text(), Some("Error: boom"));
        assert!(!error.to_string().contains("0x"));
    }
}
