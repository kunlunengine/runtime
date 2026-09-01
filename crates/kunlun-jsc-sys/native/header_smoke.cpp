#include "kunlun_jsc.h"

#include <cstdint>
#include <type_traits>

static_assert(sizeof(kunlun_jsc_array_kind) == sizeof(std::uint32_t));
static_assert(sizeof(kunlun_jsc_status) == sizeof(std::uint32_t));
static_assert(sizeof(kunlun_jsc_property_attributes) == sizeof(std::uint32_t));
static_assert(std::is_standard_layout_v<kunlun_jsc_context_group *>);
static_assert(std::is_standard_layout_v<kunlun_jsc_context *>);
static_assert(std::is_standard_layout_v<kunlun_jsc_value *>);

extern "C" kunlun_jsc_status kunlun_jsc_cpp_header_smoke(void)
{
    return KUNLUN_JSC_STATUS_OK;
}
