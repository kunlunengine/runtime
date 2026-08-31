#include "kunlun_jsc.h"

_Static_assert(sizeof(kunlun_jsc_status) == sizeof(uint32_t), "status width changed");
_Static_assert(
    sizeof(kunlun_jsc_property_attributes) == sizeof(uint32_t),
    "property attribute width changed");

_Static_assert(sizeof(kunlun_jsc_array_kind) == sizeof(uint32_t), "array kind width changed");

kunlun_jsc_status kunlun_jsc_c_header_smoke(void)
{
    kunlun_jsc_context_group *group = 0;
    kunlun_jsc_context *context = 0;
    kunlun_jsc_function_callback callback = 0;
    (void)group;
    (void)context;
    (void)callback;
    return KUNLUN_JSC_STATUS_OK;
}
