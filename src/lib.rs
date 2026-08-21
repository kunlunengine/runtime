//! Native runtime primitives for Kunlun Engine.
//!
//! The current implementation is deliberately a small embedding spike against
//! the macOS JavaScriptCore framework. It proves context ownership, exception
//! conversion, source URLs, and the inspectable-context switch before the
//! hermetic WebKit distribution and module loader land.

mod jsc;

pub use jsc::{BackendInfo, JscError, JscVm};
