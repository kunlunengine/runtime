#if defined(KUNLUN_JSC_BUNDLED)
static_assert(KUNLUN_JSC_STATUS_OK == 0,
    "KunlunMicrotaskAPI.inc uses 0 for success");
static_assert(KUNLUN_JSC_STATUS_OUT_OF_MEMORY == 2,
    "KunlunMicrotaskAPI.inc uses 2 for allocation failure");
static_assert(KUNLUN_JSC_STATUS_INVALID_STATE == 13,
    "KunlunMicrotaskAPI.inc uses 13 for invalid state");
using EngineRejectionCallback = uint32_t (*)(void *, uint64_t, uint32_t, JSValueRef, JSStringRef);
extern "C" uint32_t JSKunlunMicrotaskCheckpoint(JSGlobalContextRef, EngineRejectionCallback, void *, bool *);
extern "C" uint32_t JSKunlunMicrotasksStop(JSGlobalContextRef);
extern "C" void JSKunlunTrackDeferredPromise(JSGlobalContextRef, JSObjectRef);

namespace {
struct RejectionSink {
    kunlun_jsc_rejection_callback callback;
    void *data;
};

uint32_t rejection_bridge(void *data, uint64_t id, uint32_t transition,
    JSValueRef reason, JSStringRef source) noexcept
{
    return guard([&] {
        auto &sink = *static_cast<RejectionSink *>(data);
        return sink.callback(sink.data, id, transition,
            opaque_cast<const kunlun_jsc_value *>(reason),
            opaque_cast<const kunlun_jsc_string *>(source));
    });
}
} // namespace
#endif

kunlun_jsc_status kunlun_jsc_microtask_checkpoint(kunlun_jsc_context *context,
    kunlun_jsc_rejection_callback callback, void *user_data, uint8_t *out_pending)
{
    return guard([&] {
        if (!context || !callback || !out_pending)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_pending = 0;
#if defined(KUNLUN_JSC_BUNDLED)
        RejectionSink sink { callback, user_data };
        bool pending = false;
        auto status = JSKunlunMicrotaskCheckpoint(opaque_cast<JSGlobalContextRef>(context), rejection_bridge, &sink, &pending);
        *out_pending = pending ? 1 : 0;
        return status;
#else
        (void)user_data;
        return KUNLUN_JSC_STATUS_UNSUPPORTED;
#endif
    });
}

kunlun_jsc_status kunlun_jsc_microtasks_stop(kunlun_jsc_context *context)
{
    return guard([&] {
        if (!context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
#if defined(KUNLUN_JSC_BUNDLED)
        return JSKunlunMicrotasksStop(opaque_cast<JSGlobalContextRef>(context));
#else
        return KUNLUN_JSC_STATUS_OK;
#endif
    });
}
