// The bundled engine exports only these C extension points. Its C++ module
// implementation is compiled inside the pinned WebKit source, never against
// host-managed private headers.
#if defined(KUNLUN_JSC_BUNDLED)
using EngineModuleCallback = JSValueRef (*)(void *, JSContextRef, uint32_t, JSValueRef, JSValueRef, JSValueRef *);
extern "C" JSGlobalContextRef JSKunlunGlobalContextCreateInGroup(JSContextGroupRef);
extern "C" bool JSKunlunSetModuleLoader(JSGlobalContextRef, EngineModuleCallback, void *);
extern "C" JSObjectRef JSKunlunLoadModule(JSGlobalContextRef, JSStringRef, uint32_t, JSValueRef *);
extern "C" uint32_t JSKunlunPromiseState(JSGlobalContextRef, JSObjectRef, JSValueRef *);
#endif

#if defined(KUNLUN_JSC_TESTING)
static std::atomic<uint64_t> live_module_handles { 0 };
extern "C" uint64_t kunlun_jsc_test_live_module_handles() { return live_module_handles.load(); }
#endif

struct kunlun_jsc_module {
    JSGlobalContextRef context;
    JSStringRef key;
    JSObjectRef promise;
    bool evaluating = false;
    bool busy = false;
    std::thread::id owner = std::this_thread::get_id();

    ~kunlun_jsc_module()
    {
        if (promise) {
            JSValueUnprotect(context, promise);
#if defined(KUNLUN_JSC_TESTING)
            --live_module_handles;
#endif
        }
        JSStringRelease(key);
    }
};

struct ModuleAccess {
    kunlun_jsc_module &module;
    explicit ModuleAccess(kunlun_jsc_module &module) : module(module) { module.busy = true; }
    ~ModuleAccess() { module.busy = false; }
};

#if defined(KUNLUN_JSC_BUNDLED)
namespace {
// Function pointers have static code lifetime. No caller-owned user_data is
// stored here. The Rust trampoline retains its own isolate-local state.
thread_local std::unordered_map<JSGlobalContextRef, kunlun_jsc_module_callback> module_callbacks;

JSValueRef module_bridge(void *, JSContextRef context, uint32_t operation,
    JSValueRef key, JSValueRef referrer, JSValueRef *exception) noexcept
{
    JSValueRef result = nullptr;
    auto status = guard([&] {
        auto found = module_callbacks.find(JSContextGetGlobalContext(context));
        if (found == module_callbacks.end() || !found->second)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        auto callback = found->second;
        const kunlun_jsc_value *value = nullptr;
        const kunlun_jsc_value *error = nullptr;
        auto callback_status = callback(mutable_opaque_cast<kunlun_jsc_context *>(context), operation,
            opaque_cast<const kunlun_jsc_value *>(key),
            JSValueIsUndefined(context, referrer) ? nullptr : opaque_cast<const kunlun_jsc_value *>(referrer),
            &value, &error);
        result = opaque_cast<JSValueRef>(value);
        *exception = opaque_cast<JSValueRef>(error);
        return callback_status;
    });
    if (status != KUNLUN_JSC_STATUS_OK && !*exception)
        set_exception_message(context, exception, "Kunlun module callback failed");
    return result;
}
} // namespace
#endif

kunlun_jsc_status kunlun_jsc_modules_install(kunlun_jsc_context *context, kunlun_jsc_module_callback callback)
{
    return guard([&] {
        if (!context || !callback)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
#if defined(KUNLUN_JSC_BUNDLED)
        auto native = opaque_cast<JSGlobalContextRef>(context);
        auto inserted = module_callbacks.emplace(native, callback);
        if (!inserted.second)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        if (!JSKunlunSetModuleLoader(native, module_bridge, nullptr)) {
            module_callbacks.erase(native);
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        }
        return KUNLUN_JSC_STATUS_OK;
#else
        return KUNLUN_JSC_STATUS_UNSUPPORTED;
#endif
    });
}

kunlun_jsc_status kunlun_jsc_modules_revoke(kunlun_jsc_context *context)
{
    return guard([&] {
        if (!context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
#if defined(KUNLUN_JSC_BUNDLED)
        auto native = opaque_cast<JSGlobalContextRef>(context);
        JSKunlunSetModuleLoader(native, nullptr, nullptr);
        auto found = module_callbacks.find(native);
        if (found != module_callbacks.end())
            found->second = nullptr; // Keep a tombstone: cache policy cannot be replaced.
#endif
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_module_load(kunlun_jsc_context *context, const kunlun_jsc_string *canonical_url,
    kunlun_jsc_module **out_module, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !canonical_url || !out_module || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_module = nullptr;
        *out_exception = nullptr;
#if defined(KUNLUN_JSC_BUNDLED)
        auto native = opaque_cast<JSGlobalContextRef>(context);
        auto key = mutable_opaque_cast<JSStringRef>(canonical_url);
        auto installed = module_callbacks.find(native);
        if (installed == module_callbacks.end() || !installed->second)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        // Allocate before entering JSC so C++ allocation failure cannot strand
        // a Promise or transfer an untracked root.
        auto module = std::unique_ptr<kunlun_jsc_module>(new kunlun_jsc_module { native, JSStringRetain(key), nullptr });
        JSValueRef exception = nullptr;
        auto promise = JSKunlunLoadModule(native, key, 0, &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (!promise || exception)
            return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        JSValueProtect(native, promise);
        module->promise = promise;
#if defined(KUNLUN_JSC_TESTING)
        ++live_module_handles;
#endif
        *out_module = module.release();
        return KUNLUN_JSC_STATUS_OK;
#else
        return KUNLUN_JSC_STATUS_UNSUPPORTED;
#endif
    });
}

kunlun_jsc_status kunlun_jsc_module_evaluate(kunlun_jsc_module *module, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!module || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_exception = nullptr;
        if (module->owner != std::this_thread::get_id())
            return KUNLUN_JSC_STATUS_WRONG_THREAD;
        if (module->busy)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        ModuleAccess access(*module);
#if defined(KUNLUN_JSC_BUNDLED)
        JSValueRef result = nullptr;
        if (module->evaluating || JSKunlunPromiseState(module->context, module->promise, &result) != 1)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        JSValueRef exception = nullptr;
        auto promise = JSKunlunLoadModule(module->context, module->key, 1, &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (!promise || exception)
            return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
        JSValueProtect(module->context, promise);
        JSValueUnprotect(module->context, module->promise);
        module->promise = promise;
        module->evaluating = true;
        return KUNLUN_JSC_STATUS_OK;
#else
        return KUNLUN_JSC_STATUS_UNSUPPORTED;
#endif
    });
}

kunlun_jsc_status kunlun_jsc_module_poll(kunlun_jsc_module *module, uint32_t *out_state,
    const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!module || !out_state || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_exception = nullptr;
        *out_state = 0;
        if (module->owner != std::this_thread::get_id())
            return KUNLUN_JSC_STATUS_WRONG_THREAD;
        if (module->busy)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        ModuleAccess access(*module);
#if defined(KUNLUN_JSC_BUNDLED)
        JSValueRef result = nullptr;
        *out_state = JSKunlunPromiseState(module->context, module->promise, &result);
        if (*out_state == 2)
            *out_exception = opaque_cast<const kunlun_jsc_value *>(result);
        return *out_state <= 2 ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_WRONG_TYPE;
#else
        return KUNLUN_JSC_STATUS_UNSUPPORTED;
#endif
    });
}

kunlun_jsc_status kunlun_jsc_module_release(kunlun_jsc_module *module)
{
    return guard([&] {
        if (!module)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        if (module->owner != std::this_thread::get_id())
            return KUNLUN_JSC_STATUS_WRONG_THREAD;
        if (module->busy)
            return KUNLUN_JSC_STATUS_INVALID_STATE;
        delete module;
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_value_get_property(kunlun_jsc_context *context,
    const kunlun_jsc_value *object, const kunlun_jsc_string *name,
    const kunlun_jsc_value **out_value, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !object || !name || !out_value || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_value = nullptr;
        *out_exception = nullptr;
        auto native = opaque_cast<JSContextRef>(context);
        auto value = opaque_cast<JSValueRef>(object);
        if (!JSValueIsObject(native, value))
            return KUNLUN_JSC_STATUS_WRONG_TYPE;
        JSValueRef exception = nullptr;
        auto result = JSObjectGetProperty(native, JSValueToObject(native, value, &exception),
            mutable_opaque_cast<JSStringRef>(name), &exception);
        *out_value = opaque_cast<const kunlun_jsc_value *>(result);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OK;
    });
}
