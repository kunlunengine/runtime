#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[cfg(target_os = "macos")]
pub use macos::JscVm;
#[cfg(not(target_os = "macos"))]
pub use unsupported::JscVm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendInfo {
    pub name: &'static str,
    pub distribution: &'static str,
    pub hermetic: bool,
    pub supports_inspection: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JscError {
    UnsupportedPlatform(&'static str),
    ContextCreation,
    InvalidString,
    Exception(String),
    ValueConversion,
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
        }
    }
}

impl Error for JscError {}
