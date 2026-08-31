#ifndef KUNLUN_JSC_H
#define KUNLUN_JSC_H

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#if defined(_WIN32)
#if defined(KUNLUN_JSC_BUILDING_LIBRARY)
#define KUNLUN_JSC_API __declspec(dllexport)
#else
#define KUNLUN_JSC_API __declspec(dllimport)
#endif
#elif defined(__GNUC__) || defined(__clang__)
#define KUNLUN_JSC_API __attribute__((visibility("default")))
#else
#define KUNLUN_JSC_API
#endif

/* Incremented only for an incompatible change to this public ABI. */
#define KUNLUN_JSC_ABI_VERSION 1u

/* Stable status representation and values. */
typedef uint32_t kunlun_jsc_status;
#define KUNLUN_JSC_STATUS_OK 0u
#define KUNLUN_JSC_STATUS_INVALID_ARGUMENT 1u
#define KUNLUN_JSC_STATUS_OUT_OF_MEMORY 2u
#define KUNLUN_JSC_STATUS_JS_EXCEPTION 3u
#define KUNLUN_JSC_STATUS_BUFFER_TOO_SMALL 4u
#define KUNLUN_JSC_STATUS_INTEGER_OVERFLOW 5u
#define KUNLUN_JSC_STATUS_CALLBACK_ERROR 6u
#define KUNLUN_JSC_STATUS_CPP_EXCEPTION 7u
#define KUNLUN_JSC_STATUS_WRONG_THREAD 8u
#define KUNLUN_JSC_STATUS_WRONG_TYPE 9u
#define KUNLUN_JSC_STATUS_OUT_OF_BOUNDS 10u
#define KUNLUN_JSC_STATUS_MISALIGNED 11u

/* Stable property-attribute representation. */
typedef uint32_t kunlun_jsc_property_attributes;
#define KUNLUN_JSC_PROPERTY_ATTRIBUTE_NONE 0u

/*
 * All handles are opaque and thread-affine.
 *
 * A context group is owned after context_group_create and must be released
 * after all of its contexts. A context is owned after a successful
 * context_create/context_create_in_group and must be released exactly once.
 * A string is owned after string_create_utf8 or
 * value_to_string and must be released exactly once. Values and objects are
 * borrowed from their context; callers that retain them beyond the current
 * native call must pair value_protect with value_unprotect before releasing
 * the context.
 */
typedef struct kunlun_jsc_context_group kunlun_jsc_context_group;
typedef struct kunlun_jsc_context kunlun_jsc_context;
typedef struct kunlun_jsc_string kunlun_jsc_string;
typedef struct kunlun_jsc_value kunlun_jsc_value;
typedef kunlun_jsc_value kunlun_jsc_object;

/*
 * Host callbacks must not unwind across this boundary. Implementations return
 * OK and write a borrowed result, or return a non-OK status and may write a
 * borrowed JavaScript exception. The Rust wrapper catches panics before
 * returning. The shim catches every C++ exception around callback dispatch.
 */
typedef kunlun_jsc_status (*kunlun_jsc_function_callback)(
    kunlun_jsc_context *context,
    kunlun_jsc_object *function,
    kunlun_jsc_object *this_object,
    uint32_t argument_count,
    const kunlun_jsc_value *const *arguments,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception);

/* The caller owns user_data, keeps it alive until revoke, and roots the
 * function until revoke completes. Finalization never accesses user_data.
 * Dispatch and revocation are restricted to the creation thread. */
typedef kunlun_jsc_status (*kunlun_jsc_stateful_callback)(
    void *user_data,
    kunlun_jsc_context *context,
    uint32_t argument_count,
    const kunlun_jsc_value *const *arguments,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception);

KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_make_function_with_data(
    kunlun_jsc_context *context, const kunlun_jsc_string *name,
    kunlun_jsc_stateful_callback callback, void *user_data,
    kunlun_jsc_object **out_function, const kunlun_jsc_value **out_exception);
/* Idempotent; subsequent JS calls throw. Does not free caller-owned data. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_revoke_function(
    kunlun_jsc_context *context, kunlun_jsc_object *function);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_make_number(
    kunlun_jsc_context *context, double number, const kunlun_jsc_value **out_value);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_make_boolean(
    kunlun_jsc_context *context, uint8_t boolean, const kunlun_jsc_value **out_value);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_collect_garbage(
    kunlun_jsc_context *context);

/* Stable element kinds; these are not casts of WebKit's enum. */
typedef uint32_t kunlun_jsc_array_kind;
#define KUNLUN_JSC_ARRAY_INT8 0u
#define KUNLUN_JSC_ARRAY_UINT8 1u
#define KUNLUN_JSC_ARRAY_UINT8_CLAMPED 2u
#define KUNLUN_JSC_ARRAY_INT16 3u
#define KUNLUN_JSC_ARRAY_UINT16 4u
#define KUNLUN_JSC_ARRAY_INT32 5u
#define KUNLUN_JSC_ARRAY_UINT32 6u
#define KUNLUN_JSC_ARRAY_FLOAT32 7u
#define KUNLUN_JSC_ARRAY_FLOAT64 8u
#define KUNLUN_JSC_ARRAY_BIGINT64 9u
#define KUNLUN_JSC_ARRAY_BIGUINT64 10u

/* Copies bytes into an independent, aligned shim allocation whose ownership
 * passes to JSC. The finalizer frees only native memory and calls no Rust or
 * JSC code. On failure there is no caller cleanup obligation. Null bytes are
 * accepted only for zero length. The result is borrowed and must be rooted. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_array_buffer_create_copy(
    kunlun_jsc_context *context, const uint8_t *bytes, uint64_t length,
    kunlun_jsc_object **out_buffer, const kunlun_jsc_value **out_exception);
/* These accept fixed ArrayBuffers created by create_copy, reject detached buffers
 * with JS_EXCEPTION, and never expose a backing pointer. Offsets are in bytes.
 * Copying pins storage via JSC's public C API; a later JS transfer may throw.
 * All handles must be live/rooted and accessed on their owning isolate thread. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_array_buffer_length(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t *out_length, const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_array_buffer_read(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t offset, uint8_t *bytes, uint64_t length,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_array_buffer_write(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    uint64_t offset, const uint8_t *bytes, uint64_t length,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_typed_array_create(
    kunlun_jsc_context *context, kunlun_jsc_object *buffer,
    kunlun_jsc_array_kind kind, uint64_t byte_offset, uint64_t length,
    kunlun_jsc_object **out_array, const kunlun_jsc_value **out_exception);

KUNLUN_JSC_API kunlun_jsc_status
kunlun_jsc_context_group_create(kunlun_jsc_context_group **out_group);
KUNLUN_JSC_API kunlun_jsc_status
kunlun_jsc_context_group_release(kunlun_jsc_context_group *group);
/* Creates a non-inspectable context in an independent default group. */
KUNLUN_JSC_API kunlun_jsc_status
kunlun_jsc_context_create(kunlun_jsc_context **out_context);
/* Creates a non-inspectable context; inspection must be enabled explicitly. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_create_in_group(
    kunlun_jsc_context_group *group,
    kunlun_jsc_context **out_context);
KUNLUN_JSC_API kunlun_jsc_status
kunlun_jsc_context_release(kunlun_jsc_context *context);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_get_global_object(
    kunlun_jsc_context *context,
    kunlun_jsc_object **out_object);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_set_name(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *name);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_set_inspectable(
    kunlun_jsc_context *context,
    uint8_t inspectable);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_context_is_inspectable(
    kunlun_jsc_context *context,
    uint8_t *out_inspectable);

/* bytes may be null only when length is zero; embedded NUL bytes are rejected. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_string_create_utf8(
    const uint8_t *bytes,
    uint64_t length,
    kunlun_jsc_string **out_string);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_string_get_max_utf8_size(
    const kunlun_jsc_string *string,
    uint64_t *out_size);
/* out_written includes the trailing NUL byte. */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_string_write_utf8(
    const kunlun_jsc_string *string,
    uint8_t *buffer,
    uint64_t capacity,
    uint64_t *out_written);
KUNLUN_JSC_API kunlun_jsc_status
kunlun_jsc_string_release(kunlun_jsc_string *string);

KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_evaluate(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *source,
    kunlun_jsc_object *this_object,
    const kunlun_jsc_string *source_url,
    int32_t starting_line_number,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_call_as_function(
    kunlun_jsc_context *context,
    kunlun_jsc_object *function,
    kunlun_jsc_object *this_object,
    uint32_t argument_count,
    const kunlun_jsc_value *const *arguments,
    const kunlun_jsc_value **out_result,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_make_deferred_promise(
    kunlun_jsc_context *context,
    kunlun_jsc_object **out_promise,
    kunlun_jsc_object **out_resolve,
    kunlun_jsc_object **out_reject,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_make_error(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *message,
    kunlun_jsc_object **out_error,
    const kunlun_jsc_value **out_exception);
/*
 * The returned function is borrowed from context. The shim-owned callback
 * record is finalized with the JavaScript object; callback code must remain
 * valid until the context can no longer invoke the function.
 */
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_make_function(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *name,
    kunlun_jsc_function_callback callback,
    kunlun_jsc_object **out_function,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_object_set_property(
    kunlun_jsc_context *context,
    kunlun_jsc_object *object,
    const kunlun_jsc_string *property_name,
    const kunlun_jsc_value *value,
    kunlun_jsc_property_attributes attributes,
    const kunlun_jsc_value **out_exception);

KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_make_string(
    kunlun_jsc_context *context,
    const kunlun_jsc_string *string,
    const kunlun_jsc_value **out_value);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_make_undefined(
    kunlun_jsc_context *context,
    const kunlun_jsc_value **out_value);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_to_number(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value,
    double *out_number,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_to_string(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value,
    kunlun_jsc_string **out_string,
    const kunlun_jsc_value **out_exception);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_protect(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value);
KUNLUN_JSC_API kunlun_jsc_status kunlun_jsc_value_unprotect(
    kunlun_jsc_context *context,
    const kunlun_jsc_value *value);

#if defined(__cplusplus)
} /* extern "C" */
#endif

#endif /* KUNLUN_JSC_H */
