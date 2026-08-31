// Included by kunlun_jsc.cpp inside its C ABI block.

kunlun_jsc_status kunlun_jsc_value_make_number(
    kunlun_jsc_context *context, double number, const kunlun_jsc_value **out_value)
{
    return guard([&] {
        if (!context || !out_value)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_value = opaque_cast<const kunlun_jsc_value *>(JSValueMakeNumber(opaque_cast<JSContextRef>(context), number));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_value_make_boolean(
    kunlun_jsc_context *context, uint8_t boolean, const kunlun_jsc_value **out_value)
{
    return guard([&] {
        if (!context || !out_value || boolean > 1)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_value = opaque_cast<const kunlun_jsc_value *>(JSValueMakeBoolean(opaque_cast<JSContextRef>(context), boolean != 0));
        return KUNLUN_JSC_STATUS_OK;
    });
}

kunlun_jsc_status kunlun_jsc_context_collect_garbage(kunlun_jsc_context *context)
{
    return guard([&] {
        if (!context)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        JSGarbageCollect(opaque_cast<JSContextRef>(context));
        return KUNLUN_JSC_STATUS_OK;
    });
}

static void external_bytes_finalize(void *, void *data) noexcept
{
    // JSC invokes this once, potentially on a collector thread. This allocation
    // contains only native bytes; no Rust state or engine handle is touched.
    delete static_cast<kunlun::jsc::detail::ExternalBytes *>(data);
}

kunlun_jsc_status kunlun_jsc_array_buffer_create_copy(
    kunlun_jsc_context *context, const uint8_t *bytes, uint64_t length,
    kunlun_jsc_object **out_buffer, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !out_buffer || !out_exception || (!bytes && length))
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_buffer = nullptr;
        *out_exception = nullptr;
        size_t count;
        if (!to_size(length, count) || count > static_cast<size_t>(std::numeric_limits<ptrdiff_t>::max()))
            return KUNLUN_JSC_STATUS_INTEGER_OVERFLOW;
        auto storage = std::make_unique<kunlun::jsc::detail::ExternalBytes>(count);
        if (count)
            std::memcpy(storage->data(), bytes, count);
        auto *data = storage->data();
        // The public JSC API owns the deallocator on success AND JS exception.
        // All argument validation and shim allocations precede this transfer.
        auto *owner = storage.release();
        JSValueRef exception = nullptr;
        JSObjectRef buffer = JSObjectMakeArrayBufferWithBytesNoCopy(
            opaque_cast<JSContextRef>(context), data, count, external_bytes_finalize, owner, &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        *out_buffer = opaque_cast<kunlun_jsc_object *>(buffer);
        return buffer ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}

static kunlun_jsc_status buffer_length(
    JSContextRef ctx, JSObjectRef buffer, size_t &length, JSValueRef &exception)
{
    if (JSValueGetTypedArrayType(ctx, buffer, &exception) != kJSTypedArrayTypeArrayBuffer)
        return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_WRONG_TYPE;
    // A native zero-length view checks [[ArrayBufferData]] without invoking a
    // mutable JS property/getter. Unlike a null bytes pointer it distinguishes
    // detached buffers from legitimate empty buffers. It does not pin storage.
    if (!JSObjectMakeTypedArrayWithArrayBufferAndOffset(ctx, kJSTypedArrayTypeUint8Array, buffer, 0, 0, &exception))
        return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    length = JSObjectGetArrayBufferByteLength(ctx, buffer, &exception);
    return exception ? KUNLUN_JSC_STATUS_JS_EXCEPTION : KUNLUN_JSC_STATUS_OK;
}

kunlun_jsc_status kunlun_jsc_array_buffer_length(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t *out_length, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !buffer || !out_length || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_length = 0;
        *out_exception = nullptr;
        size_t length = 0;
        JSValueRef exception = nullptr;
        auto status = buffer_length(opaque_cast<JSContextRef>(context), opaque_cast<JSObjectRef>(buffer), length, exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (status == KUNLUN_JSC_STATUS_OK)
            *out_length = length;
        return status;
    });
}

static kunlun_jsc_status copy_buffer(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t offset, uint8_t *destination, const uint8_t *source, uint64_t length,
    const kunlun_jsc_value **out_exception, bool writing)
{
    if (!context || !buffer || !out_exception || (length && !(writing ? source : destination)))
        return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
    *out_exception = nullptr;
    auto ctx = opaque_cast<JSContextRef>(context);
    auto object = opaque_cast<JSObjectRef>(buffer);
    size_t byte_length = 0;
    JSValueRef exception = nullptr;
    auto status = buffer_length(ctx, object, byte_length, exception);
    *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
    if (status != KUNLUN_JSC_STATUS_OK)
        return status;
    if (offset > byte_length || length > byte_length - offset)
        return KUNLUN_JSC_STATUS_OUT_OF_BOUNDS;
    if (!length)
        return KUNLUN_JSC_STATUS_OK;
    // No JSC call, allocation, or user callback between getting this temporary
    // pointer and copying. Safe Rust only passes independently owned storage.
    auto *data = static_cast<uint8_t *>(JSObjectGetArrayBufferBytesPtr(ctx, object, &exception));
    if (exception) {
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        return KUNLUN_JSC_STATUS_JS_EXCEPTION;
    }
    if (!data)
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    if (writing)
        std::memcpy(data + static_cast<size_t>(offset), source, static_cast<size_t>(length));
    else
        std::memcpy(destination, data + static_cast<size_t>(offset), static_cast<size_t>(length));
    return KUNLUN_JSC_STATUS_OK;
}

kunlun_jsc_status kunlun_jsc_array_buffer_read(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t offset, uint8_t *bytes, uint64_t length, const kunlun_jsc_value **out_exception)
{
    return guard([&] { return copy_buffer(context, buffer, offset, bytes, nullptr, length, out_exception, false); });
}

kunlun_jsc_status kunlun_jsc_array_buffer_write(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t offset, const uint8_t *bytes, uint64_t length, const kunlun_jsc_value **out_exception)
{
    return guard([&] { return copy_buffer(context, buffer, offset, nullptr, bytes, length, out_exception, true); });
}

static bool array_kind(kunlun_jsc_array_kind kind, JSTypedArrayType &type, uint64_t &size)
{
    switch (kind) {
    case KUNLUN_JSC_ARRAY_INT8: type = kJSTypedArrayTypeInt8Array; size = 1; break;
    case KUNLUN_JSC_ARRAY_UINT8: type = kJSTypedArrayTypeUint8Array; size = 1; break;
    case KUNLUN_JSC_ARRAY_UINT8_CLAMPED: type = kJSTypedArrayTypeUint8ClampedArray; size = 1; break;
    case KUNLUN_JSC_ARRAY_INT16: type = kJSTypedArrayTypeInt16Array; size = 2; break;
    case KUNLUN_JSC_ARRAY_UINT16: type = kJSTypedArrayTypeUint16Array; size = 2; break;
    case KUNLUN_JSC_ARRAY_INT32: type = kJSTypedArrayTypeInt32Array; size = 4; break;
    case KUNLUN_JSC_ARRAY_UINT32: type = kJSTypedArrayTypeUint32Array; size = 4; break;
    case KUNLUN_JSC_ARRAY_FLOAT32: type = kJSTypedArrayTypeFloat32Array; size = 4; break;
    case KUNLUN_JSC_ARRAY_FLOAT64: type = kJSTypedArrayTypeFloat64Array; size = 8; break;
    case KUNLUN_JSC_ARRAY_BIGINT64: type = kJSTypedArrayTypeBigInt64Array; size = 8; break;
    case KUNLUN_JSC_ARRAY_BIGUINT64: type = kJSTypedArrayTypeBigUint64Array; size = 8; break;
    default: return false;
    }
    return true;
}

kunlun_jsc_status kunlun_jsc_typed_array_create(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    kunlun_jsc_array_kind kind, uint64_t byte_offset, uint64_t length,
    kunlun_jsc_object **out_array, const kunlun_jsc_value **out_exception)
{
    return guard([&] {
        if (!context || !buffer || !out_array || !out_exception)
            return KUNLUN_JSC_STATUS_INVALID_ARGUMENT;
        *out_array = nullptr;
        *out_exception = nullptr;
        JSTypedArrayType type;
        uint64_t size;
        if (!array_kind(kind, type, size))
            return KUNLUN_JSC_STATUS_WRONG_TYPE;
        if (byte_offset % size)
            return KUNLUN_JSC_STATUS_MISALIGNED;
        auto ctx = opaque_cast<JSContextRef>(context);
        auto object = opaque_cast<JSObjectRef>(buffer);
        size_t byte_length = 0;
        JSValueRef exception = nullptr;
        auto status = buffer_length(ctx, object, byte_length, exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (status != KUNLUN_JSC_STATUS_OK)
            return status;
        if (byte_offset > byte_length || length > (byte_length - byte_offset) / size)
            return KUNLUN_JSC_STATUS_OUT_OF_BOUNDS;
        auto array = JSObjectMakeTypedArrayWithArrayBufferAndOffset(ctx, type, object,
            static_cast<size_t>(byte_offset), static_cast<size_t>(length), &exception);
        *out_exception = opaque_cast<const kunlun_jsc_value *>(exception);
        if (exception)
            return KUNLUN_JSC_STATUS_JS_EXCEPTION;
        *out_array = opaque_cast<kunlun_jsc_object *>(array);
        return array ? KUNLUN_JSC_STATUS_OK : KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    });
}
