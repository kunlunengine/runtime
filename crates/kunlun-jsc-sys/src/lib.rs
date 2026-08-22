//! Raw JavaScriptCore C API declarations used by Kunlun.
//!
//! This crate is intentionally unsafe and contains no ownership abstractions.
//! The allowlisted declarations will later be generated from Kunlun's pinned C
//! ABI shim; the macOS system framework is the M0/M1 bootstrap backend.

#![allow(non_camel_case_types, non_snake_case)]

#[cfg(target_os = "macos")]
use std::ffi::{c_char, c_int};
use std::ffi::{c_uint, c_void};

pub type JSContextRef = *const c_void;
pub type JSGlobalContextRef = *mut c_void;
pub type JSObjectRef = *mut c_void;
pub type JSStringRef = *mut c_void;
pub type JSValueRef = *const c_void;
pub type JSPropertyAttributes = c_uint;

pub const K_JS_PROPERTY_ATTRIBUTE_NONE: JSPropertyAttributes = 0;

pub type JSObjectCallAsFunctionCallback = Option<
    unsafe extern "C" fn(
        context: JSContextRef,
        function: JSObjectRef,
        this_object: JSObjectRef,
        argument_count: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef,
>;

#[cfg(target_os = "macos")]
#[link(name = "JavaScriptCore", kind = "framework")]
unsafe extern "C" {
    pub fn JSGlobalContextCreate(global_object_class: *const c_void) -> JSGlobalContextRef;
    pub fn JSContextGetGlobalObject(context: JSContextRef) -> JSObjectRef;
    pub fn JSGlobalContextIsInspectable(context: JSGlobalContextRef) -> bool;
    pub fn JSGlobalContextRelease(context: JSGlobalContextRef);
    pub fn JSGlobalContextSetInspectable(context: JSGlobalContextRef, inspectable: bool);
    pub fn JSGlobalContextSetName(context: JSGlobalContextRef, name: JSStringRef);

    pub fn JSStringCreateWithUTF8CString(string: *const c_char) -> JSStringRef;
    pub fn JSStringGetMaximumUTF8CStringSize(string: JSStringRef) -> usize;
    pub fn JSStringGetUTF8CString(string: JSStringRef, buffer: *mut c_char, size: usize) -> usize;
    pub fn JSStringRelease(string: JSStringRef);

    pub fn JSEvaluateScript(
        context: JSContextRef,
        script: JSStringRef,
        this_object: JSObjectRef,
        source_url: JSStringRef,
        starting_line_number: c_int,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectCallAsFunction(
        context: JSContextRef,
        object: JSObjectRef,
        this_object: JSObjectRef,
        argument_count: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectMakeDeferredPromise(
        context: JSContextRef,
        resolve: *mut JSObjectRef,
        reject: *mut JSObjectRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeFunctionWithCallback(
        context: JSContextRef,
        name: JSStringRef,
        callback: JSObjectCallAsFunctionCallback,
    ) -> JSObjectRef;
    pub fn JSObjectSetProperty(
        context: JSContextRef,
        object: JSObjectRef,
        property_name: JSStringRef,
        value: JSValueRef,
        attributes: JSPropertyAttributes,
        exception: *mut JSValueRef,
    );
    pub fn JSValueMakeString(context: JSContextRef, string: JSStringRef) -> JSValueRef;
    pub fn JSValueMakeUndefined(context: JSContextRef) -> JSValueRef;
    pub fn JSValueProtect(context: JSContextRef, value: JSValueRef);
    pub fn JSValueToNumber(
        context: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> f64;
    pub fn JSValueToStringCopy(
        context: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSStringRef;
    pub fn JSValueUnprotect(context: JSContextRef, value: JSValueRef);
}
