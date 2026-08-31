#include "kunlun_jsc.h"
#include "external_bytes.hpp"

#include <cassert>
#include <cstring>
#include <limits>
#include <stdexcept>
#include <thread>
#include <vector>

using kunlun::jsc::detail::ExternalBytes;

struct Context {
    kunlun_jsc_context_group *group = nullptr;
    kunlun_jsc_context *raw = nullptr;
    Context()
    {
        assert(kunlun_jsc_context_group_create(&group) == 0);
        assert(kunlun_jsc_context_create_in_group(group, &raw) == 0);
    }
    ~Context()
    {
        assert(kunlun_jsc_context_release(raw) == 0);
        assert(kunlun_jsc_context_group_release(group) == 0);
    }
};

struct String {
    kunlun_jsc_string *raw = nullptr;
    explicit String(const char *text)
    {
        assert(kunlun_jsc_string_create_utf8(reinterpret_cast<const uint8_t *>(text), std::strlen(text), &raw) == 0);
    }
    ~String() { assert(kunlun_jsc_string_release(raw) == 0); }
};

static void evaluate(Context &ctx, const char *source)
{
    String script(source);
    String url("test:///native-ownership.js");
    const kunlun_jsc_value *value = nullptr, *exception = nullptr;
    assert(kunlun_jsc_evaluate(ctx.raw, script.raw, nullptr, url.raw, 1, &value, &exception) == 0);
    assert(value && !exception);
}

static kunlun_jsc_status callback(void *data, kunlun_jsc_context *context,
    uint32_t, const kunlun_jsc_value *const *, const kunlun_jsc_value **result,
    const kunlun_jsc_value **)
{
    ++*static_cast<unsigned *>(data);
    return kunlun_jsc_value_make_number(context, 42, result);
}

static kunlun_jsc_status throwing_callback(void *, kunlun_jsc_context *,
    uint32_t, const kunlun_jsc_value *const *, const kunlun_jsc_value **,
    const kunlun_jsc_value **)
{
    throw std::runtime_error("native callback exception");
}

int main()
{
    // Exercise idempotent cleanup, including competing cleanup paths. State is
    // retained until all threads join; no reader races storage reclamation.
    {
        ExternalBytes bytes(32);
        std::vector<std::thread> threads;
        for (int i = 0; i < 8; ++i)
            threads.emplace_back([&] { bytes.release(); bytes.release(); });
        for (auto &thread : threads)
            thread.join();
        assert(!bytes.data());
    }
    assert(ExternalBytes::live_allocations == 0);
    for (int cycle = 0; cycle < 32; ++cycle) {
        Context ctx;
        kunlun_jsc_object *global = nullptr;
        assert(kunlun_jsc_context_get_global_object(ctx.raw, &global) == 0);
        const kunlun_jsc_value *exception = nullptr;
        String key("buffer");
        for (uint64_t size : { 0, 16 }) {
            uint8_t input[16] = { 1, 2, 3, 4 };
            kunlun_jsc_object *buffer = nullptr;
            assert(kunlun_jsc_array_buffer_create_copy(ctx.raw, input, size, &buffer, &exception) == 0);
            assert(kunlun_jsc_value_protect(ctx.raw, buffer) == 0);
            assert(kunlun_jsc_object_set_property(ctx.raw, global, key.raw, buffer, 0, &exception) == 0);
            uint64_t length = 99;
            assert(kunlun_jsc_array_buffer_length(ctx.raw, buffer, &length, &exception) == 0 && length == size);
            kunlun_jsc_object *view = nullptr;
            assert(kunlun_jsc_typed_array_create(ctx.raw, buffer, KUNLUN_JSC_ARRAY_FLOAT64, size, 0, &view, &exception) == 0);
            assert(kunlun_jsc_typed_array_create(ctx.raw, buffer, 99, 0, 0, &view, &exception) == KUNLUN_JSC_STATUS_WRONG_TYPE);
            assert(kunlun_jsc_typed_array_create(ctx.raw, buffer, KUNLUN_JSC_ARRAY_UINT32, 1, 0, &view, &exception) == KUNLUN_JSC_STATUS_MISALIGNED);
            assert(kunlun_jsc_array_buffer_read(ctx.raw, buffer, size + 1, nullptr, 0, &exception) == KUNLUN_JSC_STATUS_OUT_OF_BOUNDS);
            evaluate(ctx, "globalThis.moved = buffer.transfer(); if (!buffer.detached) throw Error('not detached')");
            assert(kunlun_jsc_array_buffer_read(ctx.raw, buffer, 0, nullptr, 0, &exception) == KUNLUN_JSC_STATUS_JS_EXCEPTION);
            assert(kunlun_jsc_value_unprotect(ctx.raw, buffer) == 0);
        }
        for (int n = 0; n < 64; ++n) {
            uint8_t input[16] = { 1, 2, 3, 4 };
            kunlun_jsc_object *buffer = nullptr;
            assert(kunlun_jsc_array_buffer_create_copy(ctx.raw, input, sizeof(input), &buffer, &exception) == 0);
            assert(kunlun_jsc_value_protect(ctx.raw, buffer) == 0);
            uint8_t output[4] = {};
            assert(kunlun_jsc_array_buffer_read(ctx.raw, buffer, 0, output, sizeof(output), &exception) == 0);
            assert(std::memcmp(input, output, sizeof(output)) == 0);
            assert(kunlun_jsc_array_buffer_write(ctx.raw, buffer, 12, output, sizeof(output), &exception) == 0);
            assert(kunlun_jsc_array_buffer_write(ctx.raw, buffer, 13, output, sizeof(output), &exception) == KUNLUN_JSC_STATUS_OUT_OF_BOUNDS);
            assert(kunlun_jsc_array_buffer_read(ctx.raw, buffer, 0, output, UINT64_MAX, &exception) == KUNLUN_JSC_STATUS_OUT_OF_BOUNDS);
            assert(kunlun_jsc_value_unprotect(ctx.raw, buffer) == 0);
            assert(kunlun_jsc_context_collect_garbage(ctx.raw) == 0);
        }
        auto before_failure = ExternalBytes::live_allocations.load();
        kunlun_jsc_object *unused = nullptr;
        assert(kunlun_jsc_array_buffer_create_copy(ctx.raw, nullptr, 1, &unused, &exception) == KUNLUN_JSC_STATUS_INVALID_ARGUMENT);
        uint8_t byte = 0;
        assert(kunlun_jsc_array_buffer_create_copy(ctx.raw, &byte, UINT64_MAX, &unused, &exception) == KUNLUN_JSC_STATUS_INTEGER_OVERFLOW);
        assert(ExternalBytes::live_allocations == before_failure);
        unsigned calls = 0;
        kunlun_jsc_object *function = nullptr;
        String name("host");
        assert(kunlun_jsc_object_make_function_with_data(ctx.raw, name.raw, callback, &calls, &function, &exception) == 0);
        assert(kunlun_jsc_value_protect(ctx.raw, function) == 0);
        assert(kunlun_jsc_object_set_property(ctx.raw, global, name.raw, function, 0, &exception) == 0);
        evaluate(ctx, "if (host() !== 42) throw Error('wrong result')");
        std::thread other([&] {
            const kunlun_jsc_value *result = nullptr, *error = nullptr;
            assert(kunlun_jsc_object_call_as_function(ctx.raw, function, nullptr, 0, nullptr, &result, &error) == KUNLUN_JSC_STATUS_JS_EXCEPTION);
            assert(kunlun_jsc_object_revoke_function(ctx.raw, function) == KUNLUN_JSC_STATUS_WRONG_THREAD);
        });
        other.join();
        assert(calls == 1);
        assert(kunlun_jsc_object_revoke_function(ctx.raw, function) == 0);
        assert(kunlun_jsc_object_revoke_function(ctx.raw, function) == 0);
        evaluate(ctx, "var caught = false; try { host() } catch (e) { caught = true } if (!caught) throw Error('not revoked')");
        assert(calls == 1);
        assert(kunlun_jsc_value_unprotect(ctx.raw, function) == 0);
        assert(kunlun_jsc_object_make_function_with_data(ctx.raw, name.raw, throwing_callback, &calls, &function, &exception) == 0);
        assert(kunlun_jsc_object_set_property(ctx.raw, global, name.raw, function, 0, &exception) == 0);
        evaluate(ctx, "var caught = false; try { host() } catch (e) { caught = true } if (!caught) throw Error('C++ exception escaped')");
        assert(kunlun_jsc_object_make_function_with_data(ctx.raw, name.raw, nullptr, &calls, &unused, &exception) == KUNLUN_JSC_STATUS_INVALID_ARGUMENT);
    }
    assert(ExternalBytes::live_allocations == 0);
}
