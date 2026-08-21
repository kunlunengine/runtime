use super::{BackendInfo, JscError};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr;
use std::rc::Rc;

type JSContextRef = *const c_void;
type JSGlobalContextRef = *mut c_void;
type JSObjectRef = *mut c_void;
type JSStringRef = *mut c_void;
type JSValueRef = *const c_void;

#[link(name = "JavaScriptCore", kind = "framework")]
unsafe extern "C" {
    fn JSGlobalContextCreate(global_object_class: *const c_void) -> JSGlobalContextRef;
    fn JSGlobalContextIsInspectable(context: JSGlobalContextRef) -> bool;
    fn JSGlobalContextRelease(context: JSGlobalContextRef);
    fn JSGlobalContextSetInspectable(context: JSGlobalContextRef, inspectable: bool);
    fn JSGlobalContextSetName(context: JSGlobalContextRef, name: JSStringRef);

    fn JSStringCreateWithUTF8CString(string: *const c_char) -> JSStringRef;
    fn JSStringGetMaximumUTF8CStringSize(string: JSStringRef) -> usize;
    fn JSStringGetUTF8CString(string: JSStringRef, buffer: *mut c_char, size: usize) -> usize;
    fn JSStringRelease(string: JSStringRef);

    fn JSEvaluateScript(
        context: JSContextRef,
        script: JSStringRef,
        this_object: JSObjectRef,
        source_url: JSStringRef,
        starting_line_number: c_int,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSValueToStringCopy(
        context: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSStringRef;
}

/// An owned JavaScriptCore global context.
///
/// `Rc` in the marker intentionally makes the VM `!Send + !Sync`. JavaScript
/// values and host callbacks must stay on their isolate thread; cross-isolate
/// communication belongs in the runtime message layer.
pub struct JscVm {
    context: JSGlobalContextRef,
    _thread_affinity: PhantomData<Rc<()>>,
}

impl JscVm {
    pub const fn backend_info() -> BackendInfo {
        BackendInfo {
            name: "JavaScriptCore",
            distribution: "macOS system framework (bootstrap only)",
            hermetic: false,
            supports_inspection: true,
        }
    }

    pub fn new(name: &str) -> Result<Self, JscError> {
        let context = unsafe { JSGlobalContextCreate(ptr::null()) };
        if context.is_null() {
            return Err(JscError::ContextCreation);
        }

        let vm = Self {
            context,
            _thread_affinity: PhantomData,
        };
        vm.set_name(name)?;
        Ok(vm)
    }

    pub fn set_name(&self, name: &str) -> Result<(), JscError> {
        let name = OwnedJsString::new(name)?;
        unsafe { JSGlobalContextSetName(self.context, name.raw) };
        Ok(())
    }

    /// Makes this context visible to Web Inspector on supported Apple hosts.
    ///
    /// This is a local bootstrap path, not Kunlun's cross-platform inspector
    /// transport. The latter is specified in `docs/devtools.md`.
    pub fn set_inspectable(&self, inspectable: bool) {
        unsafe { JSGlobalContextSetInspectable(self.context, inspectable) };
    }

    pub fn is_inspectable(&self) -> bool {
        unsafe { JSGlobalContextIsInspectable(self.context) }
    }

    pub fn evaluate(&mut self, source: &str, source_url: &str) -> Result<String, JscError> {
        let source = OwnedJsString::new(source)?;
        let source_url = OwnedJsString::new(source_url)?;
        let mut exception = ptr::null();
        let value = unsafe {
            JSEvaluateScript(
                self.context,
                source.raw,
                ptr::null_mut(),
                source_url.raw,
                1,
                &mut exception,
            )
        };

        if !exception.is_null() {
            return Err(JscError::Exception(self.value_to_string(exception)?));
        }
        if value.is_null() {
            return Err(JscError::ValueConversion);
        }
        self.value_to_string(value)
    }

    fn value_to_string(&self, value: JSValueRef) -> Result<String, JscError> {
        let mut exception = ptr::null();
        let string = unsafe { JSValueToStringCopy(self.context, value, &mut exception) };
        if !exception.is_null() || string.is_null() {
            return Err(JscError::ValueConversion);
        }
        let string = OwnedJsString { raw: string };
        string.to_utf8()
    }
}

impl Drop for JscVm {
    fn drop(&mut self) {
        unsafe { JSGlobalContextRelease(self.context) };
    }
}

struct OwnedJsString {
    raw: JSStringRef,
}

impl OwnedJsString {
    fn new(value: &str) -> Result<Self, JscError> {
        let value = CString::new(value).map_err(|_| JscError::InvalidString)?;
        let raw = unsafe { JSStringCreateWithUTF8CString(value.as_ptr()) };
        if raw.is_null() {
            return Err(JscError::ValueConversion);
        }
        Ok(Self { raw })
    }

    fn to_utf8(&self) -> Result<String, JscError> {
        let capacity = unsafe { JSStringGetMaximumUTF8CStringSize(self.raw) };
        if capacity == 0 {
            return Err(JscError::ValueConversion);
        }

        let mut buffer = vec![0_u8; capacity];
        let written = unsafe {
            JSStringGetUTF8CString(self.raw, buffer.as_mut_ptr().cast::<c_char>(), capacity)
        };
        if written == 0 {
            return Err(JscError::ValueConversion);
        }

        let c_string = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) };
        c_string
            .to_str()
            .map(str::to_owned)
            .map_err(|_| JscError::ValueConversion)
    }
}

impl Drop for OwnedJsString {
    fn drop(&mut self) {
        unsafe { JSStringRelease(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_javascript() {
        let mut vm = JscVm::new("kunlun-test").expect("create VM");
        assert_eq!(vm.evaluate("21 * 2", "test:///eval.js").unwrap(), "42");
    }

    #[test]
    fn returns_javascript_exceptions() {
        let mut vm = JscVm::new("kunlun-test").expect("create VM");
        let error = vm
            .evaluate("throw new Error('boom')", "test:///exception.js")
            .unwrap_err();
        assert!(matches!(error, JscError::Exception(message) if message.contains("boom")));
    }

    #[test]
    fn toggles_web_inspector_visibility() {
        let vm = JscVm::new("kunlun-inspector-test").expect("create VM");
        assert!(!vm.is_inspectable());
        vm.set_inspectable(true);
        assert!(vm.is_inspectable());
        vm.set_inspectable(false);
        assert!(!vm.is_inspectable());
    }
}
