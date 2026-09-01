#include "kunlun_jsc.h"
#include "exception_boundary.hpp"
#include "external_bytes.hpp"

#include <JavaScriptCore/JavaScript.h>

#include <cstring>
#include <limits>
#include <memory>
#include <thread>
#include <vector>

namespace {

using kunlun::jsc::detail::guard;

static_assert(sizeof(int) == sizeof(int32_t));
static_assert(sizeof(JSPropertyAttributes) == sizeof(kunlun_jsc_property_attributes));

template <typename To, typename From>
To opaque_cast(From value) noexcept
{
    return reinterpret_cast<To>(value);
}

template <typename To, typename From>
To mutable_opaque_cast(const From *value) noexcept
{
    return reinterpret_cast<To>(const_cast<From *>(value));
}

bool to_size(uint64_t value, size_t &out) noexcept
{
    if (value > static_cast<uint64_t>(std::numeric_limits<size_t>::max()))
        return false;
    out = static_cast<size_t>(value);
    return true;
}

JSObjectRef make_error(JSContextRef context, JSStringRef message, JSValueRef *exception)
{
    JSValueRef argument = JSValueMakeString(context, message);
    if (!argument)
        return nullptr;
    return JSObjectMakeError(context, 1, &argument, exception);
}

void set_exception_message(JSContextRef context, JSValueRef *exception, const char *message) noexcept
{
    if (!exception)
        return;
    *exception = nullptr;

    try {
        JSStringRef string = JSStringCreateWithUTF8CString(message);
        if (!string)
            return;
        JSValueRef creation_exception = nullptr;
        JSObjectRef error = make_error(context, string, &creation_exception);
        JSStringRelease(string);
        *exception = creation_exception ? creation_exception : error;
    } catch (...) {
        *exception = nullptr;
    }
}

struct CallbackState {
    kunlun_jsc_function_callback callback = nullptr;
    kunlun_jsc_stateful_callback stateful_callback = nullptr;
    void *user_data = nullptr; // Borrowed, never dereferenced by the finalizer.
    std::thread::id owner = std::this_thread::get_id();
};

JSValueRef callback_bridge(JSContextRef, JSObjectRef, JSObjectRef, size_t, const JSValueRef[], JSValueRef *);
void callback_finalize(JSObjectRef);

JSClassRef callback_class()
{
    // Process-lifetime class metadata; JSC instances own their own references.
    static JSClassRef klass = [] {
        JSClassDefinition definition = kJSClassDefinitionEmpty;
        definition.finalize = callback_finalize;
        definition.callAsFunction = callback_bridge;
        return JSClassCreate(&definition);
    }();
    return klass;
}

struct ScopedRoot {
    JSContextRef context;
    JSValueRef value;
    ScopedRoot(JSContextRef context, JSValueRef value) : context(context), value(value)
    {
        JSValueProtect(context, value);
    }
    ~ScopedRoot() { JSValueUnprotect(context, value); }
};

JSValueRef callback_bridge(
    JSContextRef context,
    JSObjectRef function,
    JSObjectRef this_object,
    size_t argument_count,
    const JSValueRef arguments[],
    JSValueRef *exception)
{
    try {
        auto *state = static_cast<CallbackState *>(JSObjectGetPrivate(function));
        if (!state || (!state->callback && !state->stateful_callback)) {
            set_exception_message(context, exception, "Kunlun callback state is unavailable");
            return nullptr;
        }
        if (state->owner != std::this_thread::get_id()) {
            set_exception_message(context, exception, "Kunlun callback invoked on the wrong thread");
            return nullptr;
        }
        if (argument_count > std::numeric_limits<uint32_t>::max()) {
            set_exception_message(context, exception, "Kunlun callback argument count overflowed");
            return nullptr;
        }

        const kunlun_jsc_value *result = nullptr;
        const kunlun_jsc_value *callback_exception = nullptr;
        auto *raw_context = opaque_cast<kunlun_jsc_context *>(const_cast<OpaqueJSContext *>(context));
        auto raw_arguments = opaque_cast<const kunlun_jsc_value *const *>(arguments);
        kunlun_jsc_status status = state->stateful_callback
            ? state->stateful_callback(state->user_data, raw_context,
                static_cast<uint32_t>(argument_count), raw_arguments, &result, &callback_exception)
            : state->callback(raw_context, opaque_cast<kunlun_jsc_object *>(function),
                opaque_cast<kunlun_jsc_object *>(this_object), static_cast<uint32_t>(argument_count),
                raw_arguments, &result, &callback_exception);

        if (callback_exception) {
            if (exception)
                *exception = opaque_cast<JSValueRef>(callback_exception);
            return nullptr;
        }
        if (status != KUNLUN_JSC_STATUS_OK) {
            set_exception_message(context, exception, "Kunlun host callback failed");
            return nullptr;
        }
        if (!result) {
            set_exception_message(context, exception, "Kunlun host callback returned no value");
            return nullptr;
        }
        return opaque_cast<JSValueRef>(result);
    } catch (...) {
        set_exception_message(context, exception, "Kunlun C++ callback bridge caught an exception");
        return nullptr;
    }
}

void callback_finalize(JSObjectRef function)
{
    try {
        delete static_cast<CallbackState *>(JSObjectGetPrivate(function));
    } catch (...) {
        // The state has a trivial destructor. This final catch makes the JSC
        // finalizer boundary explicitly non-throwing if that changes later.
    }
}

} // namespace

extern "C" {

kunlun_jsc_status kunlun_jsc_context_group_create(kunlun_jsc_context_group **out_group)
{
    return guard([&] {
        if (!out_group)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_group = nullptr;
        JSContextGroupRef group = JSContextGroupCreate();
        if (!group)
            return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        *out_group = mutable_opaque_cast<kunlun_jsc_context_group *>(group);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_group_release(kunlun_jsc_context_group *group)
{
    return guard([&] {
        if (!group)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSContextGroupRelease(opaque_cast<JSContextGroupRef>(group));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_create(kunlun_jsc_context **out_context)
{
    return guard([&] {
        if (!out_context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_context = nullptr;
        JSGlobalContextRef context = JSGlobalContextCreate(nullptr);
        if (!context)
            return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        // WebKit enables inspection by default on non-Cocoa platforms. Keep the
        // Kunlun embedding contract secure and consistent across platforms.
        JSGlobalContextSetInspectable(context, false);
        *out_context = opaque_cast<kunlun_jsc_context *>(context);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_create_in_group(
    kunlun_jsc_context_group *group,
    kunlun_jsc_context **out_context)
{
    return guard([&] {
        if (!group || !out_context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_context = nullptr;
        JSGlobalContextRef context = JSGlobalContextCreateInGroup(
            opaque_cast<JSContextGroupRef>(group), nullptr);
        if (!context)
            return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        // Match kunlun_jsc_context_create: inspection is always opt-in.
        JSGlobalContextSetInspectable(context, false);
        *out_context = opaque_cast<kunlun_jsc_context *>(context);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_release(kunlun_jsc_context *context)
{
    return guard([&] {
        if (!context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSGlobalContextRelease(opaque_cast<JSGlobalContextRef>(context));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_get_global_object(
    kunlun_jsc_context *context,
    kunlun_jsc_object **out_object)
{
    return guard([&] {
        if (!context || !out_object)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_object = opaque_cast<kunlun_jsc_object *>(
            JSContextGetGlobalObject(opaque_cast<JSContextRef>(context)));
        return *out_object ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_context_set_name(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *name)
{
    return guard([&] {
        if (!context || !name)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSGlobalContextSetName(
            opaque_cast<JSGlobalContextRef>(context), mutable_opaque_cast<JSStringRef>(name));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_set_inspectable(
    kunlun_jsc_context *context,
    uint8_t inspectable)
{
    return guard([&] {
        if (!context || inspectable > 1)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSGlobalContextSetInspectable(
            opaque_cast<JSGlobalContextRef>(context), inspectable != 0);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_is_inspectable(
    kunlun_jsc_context *context,
    uint8_t *out_inspectable)
{
    return guard([&] {
        if (!context || !out_inspectable)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_inspectable = JSGlobalContextIsInspectable(
                               opaque_cast<JSGlobalContextRef>(context))
            ? 1
            : 0;
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_string_create_utf8(
    const uint8_t *bytes,
    uint64_t length,
    kunlun_jsc_string **out_string)
{
    return guard([&] {
        if (!out_string || (!bytes && length != 0))
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_string = nullptr;

        size_t native_length = 0;
        if (!to_size(length, native_length) || native_length == std::numeric_limits<size_t>::max())
            return KUNLUN_JSC_STATUS_INTEGER_OVERFLOW;
        if (native_length != 0 && std::memchr(bytes, '\0', native_length))
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;

        std::vector<char> terminated(native_length + 1, '\0');
        if (native_length != 0)
            std::memcpy(terminated.data(), bytes, native_length);
        JSStringRef string = JSStringCreateWithUTF8CString(terminated.data());
        if (!string)
            return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        *out_string = opaque_cast<kunlun_jsc_string *>(string);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_string_get_max_utf8_size(
    const kunlun_jsc_string *string,
    uint64_t *out_size)
{
    return guard([&] {
        if (!string || !out_size)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        size_t size = JSStringGetMaximumUTF8CStringSize(mutable_opaque_cast<JSStringRef>(string));
        *out_size = static_cast<uint64_t>(size);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_string_write_utf8(
    const kunlun_jsc_string *string,
    uint8_t *buffer,
    uint64_t capacity,
    uint64_t *out_written)
{
    return guard([&] {
        if (!string || !out_written || (!buffer && capacity != 0))
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_written = 0;
        size_t native_capacity = 0;
        if (!to_size(capacity, native_capacity))
            return KUNLUN_JSC_STATUS_INTEGER_OVERFLOW;
        size_t required = JSStringGetMaximumUTF8CStringSize(mutable_opaque_cast<JSStringRef>(string));
        if (native_capacity < required)
            return KUNLUN_JSC_STATUS_BUFFER_TOO_SMALL;
        size_t written = JSStringGetUTF8CString(
            mutable_opaque_cast<JSStringRef>(string),
            reinterpret_cast<char *>(buffer),
            native_capacity);
        if (!written)
            return KUNLUN_JSC_STATUS_BUFFER_TOO_SMALL;
        *out_written = static_cast<uint64_t>(written);
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_string_release(kunlun_jsc_string *string)
{
    return guard([&] {
        if (!string)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSStringRelease(opaque_cast<JSStringRef>(string));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_evaluate(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *source,
    kunlun_jsc_object *this_object,
    const kunlun_jsc_string *source_url,
    int32_t starting_line_number,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !source || !out_result || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_result = nullptr;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        JSValueRef result = JSEvaluateScript(
            opaque_cast<JSContextRef>(context),
            mutable_opaque_cast<JSStringRef>(source),
            opaque_cast<JSObjectRef>(this_object),
            mutable_opaque_cast<JSStringRef>(source_url),
            starting_line_number,
            &exception);
        *out_result = opaque_cast<const kunlun_jsc_value *>(result);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        return result ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_object_call_as_function(
    kunlun_jsc_context *context,
    kunlun_jsc_object *function,
    kunlun_jsc_object *this_object,
    uint32_t argument_count,
    const kunlun_jsc_value *const *arguments,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !function || !out_result || !out_exception
            || (argument_count != 0 && !arguments))
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_result = nullptr;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        JSValueRef result = JSObjectCallAsFunction(
            opaque_cast<JSContextRef>(context),
            opaque_cast<JSObjectRef>(function),
            opaque_cast<JSObjectRef>(this_object),
            static_cast<size_t>(argument_count),
            opaque_cast<const JSValueRef *>(arguments),
            &exception);
        *out_result = opaque_cast<const kunlun_jsc_value *>(result);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        return result ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_object_make_deferred_promise(
    kunlun_jsc_context *context,
    kunlun_jsc_object **out_promise,
    kunlun_jsc_object **out_resolve,
    kunlun_jsc_object **out_reject,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !out_promise || !out_resolve || !out_reject || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_promise = nullptr;
        *out_resolve = nullptr;
        *out_reject = nullptr;
        *out_exception = nullptr;
        JSObjectRef resolve = nullptr;
        JSObjectRef reject = nullptr;
        JSValueRef exception = nullptr;
        JSObjectRef promise = JSObjectMakeDeferredPromise(
            opaque_cast<JSContextRef>(context), &resolve, &reject, &exception);
        *out_promise = opaque_cast<kunlun_jsc_object *>(promise);
        *out_resolve = opaque_cast<kunlun_jsc_object *>(resolve);
        *out_reject = opaque_cast<kunlun_jsc_object *>(reject);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        return promise && resolve && reject ? KUNLUN_JSC_STATUS_OK
                                           : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_object_make_error(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *message,
    kunlun_jsc_object **out_error,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !message || !out_error || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_error = nullptr;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        JSObjectRef error = make_error(
            opaque_cast<JSContextRef>(context),
            mutable_opaque_cast<JSStringRef>(message),
            &exception);
        *out_error = opaque_cast<kunlun_jsc_object *>(error);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        return error ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

// Called only inside an exported exception guard. On failure the object is
// never published and its finalizer only destroys the native record.
static kunlun_jsc_status make_function(
    kunlun_jsc_context *context, const kunlun_jsc_string *name,
    std::unique_ptr<CallbackState> state, kunlun_jsc_object **out_function,
    const kunlun_jsc_value **out_exception)
{
    if (!context || !name || !out_function || !out_exception)
        return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
    *out_function = nullptr;
    *out_exception = nullptr;
    auto ctx = opaque_cast<JSContextRef>(context);
    JSClassRef klass = callback_class();
    if (!klass)
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    JSObjectRef function = JSObjectMake(ctx, klass, state.get());
    if (!function)
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    state.release(); // JS owns only the native record, never user_data.
    ScopedRoot root(ctx, function);
    JSStringRef name_key = JSStringCreateWithUTF8CString("name");
    if (!name_key)
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    auto release_key = [](OpaqueJSString *key) { JSStringRelease(key); };
    std::unique_ptr<OpaqueJSString, decltype(release_key)> key(name_key, release_key);
    JSValueRef exception = nullptr;
    JSValueRef name_value = JSValueMakeString(ctx, mutable_opaque_cast<JSStringRef>(name));
    if (!name_value)
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    JSObjectSetProperty(ctx, function, name_key, name_value,
        kJSPropertyAttributeReadOnly | kJSPropertyAttributeDontEnum | kJSPropertyAttributeDontDelete,
        &exception);
    if (exception) {
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        return KUNLUN_JSC_STATUS_JS_EXCEPTION;
    }
    *out_function = opaque_cast<kunlun_jsc_object *>(function);
    return KUNLUN_JSC_STATUS_OK;
}

kunlun_jsc_status kunlun_jsc_object_make_function(
    kunlun_jsc_context *context, const kunlun_jsc_string *name,
    kunlun_jsc_function_callback callback, kunlun_jsc_object **out_function,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!callback)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        auto state = std::make_unique<CallbackState>();
        state->callback = callback;
        return make_function(context, name, std::move(state), out_function, out_exception);
    });
}

kunlun_jsc_status kunlun_jsc_object_make_function_with_data(
    kunlun_jsc_context *context, const kunlun_jsc_string *name,
    kunlun_jsc_stateful_callback callback, void *user_data,
    kunlun_jsc_object **out_function, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!callback || !user_data)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        auto state = std::make_unique<CallbackState>();
        state->stateful_callback = callback;
        state->user_data = user_data;
        return make_function(context, name, std::move(state), out_function, out_exception);
    });
}

kunlun_jsc_status kunlun_jsc_object_revoke_function(
    kunlun_jsc_context *context, kunlun_jsc_object *function)
{
    return guard([&] {
        if (!context || !function)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        if (!JSValueIsObjectOfClass(opaque_cast<JSContextRef>(context),
                opaque_cast<JSValueRef>(function), callback_class()))
            return KUNLUN_JSC_STATUS_WRONG_TYPE;
        auto *state = static_cast<CallbackState *>(JSObjectGetPrivate(opaque_cast<JSObjectRef>(function)));
        if (state->owner != std::this_thread::get_id())
            return KUNLUN_JSC_STATUS_WRONG_THREAD;
        state->callback = nullptr;
        state->stateful_callback = nullptr;
        state->user_data = nullptr;
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_object_set_property(
    kunlun_jsc_context *context,
    kunlun_jsc_object *object,
    const kunlun_jsc_string *property_name,
    const kunlun_jsc_value *value,
    kunlun_jsc_property_attributes attributes,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !object || !property_name || !value || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        JSObjectSetProperty(
            opaque_cast<JSContextRef>(context),
            opaque_cast<JSObjectRef>(object),
            mutable_opaque_cast<JSStringRef>(property_name),
            opaque_cast<JSValueRef>(value),
            static_cast<JSPropertyAttributes>(attributes),
            &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_value_make_string(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *string,
    const kunlun_jsc_value **out_value)
{
    return guard([&] {
        if (!context || !string || !out_value)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_value = opaque_cast<const kunlun_jsc_value *>(JSValueMakeString(
            opaque_cast<JSContextRef>(context), mutable_opaque_cast<JSStringRef>(string)));
        return *out_value ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_value_make_undefined(
    kunlun_jsc_context *context,
    const kunlun_jsc_value **out_value)
{
    return guard([&] {
        if (!context || !out_value)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_value = opaque_cast<const kunlun_jsc_value *>(
            JSValueMakeUndefined(opaque_cast<JSContextRef>(context)));
        return *out_value ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_value_to_number(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value,
    double *out_number,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !value || !out_number || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        *out_number = JSValueToNumber(
            opaque_cast<JSContextRef>(context), opaque_cast<JSValueRef>(value), &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_value_to_string(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value,
    kunlun_jsc_string **out_string,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !value || !out_string || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_string = nullptr;
        *out_exception = nullptr;
        JSValueRef exception = nullptr;
        JSStringRef string = JSValueToStringCopy(
            opaque_cast<JSContextRef>(context), opaque_cast<JSValueRef>(value), &exception);
        *out_string = opaque_cast<kunlun_jsc_string *>(string);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        return string ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

kunlun_jsc_status kunlun_jsc_value_protect(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value)
{
    return guard([&] {
        if (!context || !value)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSValueProtect(opaque_cast<JSContextRef>(context), opaque_cast<JSValueRef>(value));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_value_unprotect(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value)
{
    return guard([&] {
        if (!context || !value)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSValueUnprotect(opaque_cast<JSContextRef>(context), opaque_cast<JSValueRef>(value));
        return KUNLUN_JSC_STATUS_OK;
    });
}

#include "buffers.inc"

} // extern "C"
