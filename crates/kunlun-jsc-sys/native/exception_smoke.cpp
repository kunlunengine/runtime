#include "exception_boundary.hpp"

#include <new>

#if defined(__GNUC__) || defined(__clang__)
#define KUNLUN_JSC_INTERNAL __attribute__((visibility("hidden")))
#else
#define KUNLUN_JSC_INTERNAL
#endif

extern "C" KUNLUN_JSC_INTERNAL kunlun_jsc_status
kunlun_jsc_internal_test_bad_alloc_boundary(void) noexcept
{
    return kunlun::jsc::detail::guard([]() -> kunlun_jsc_status { throw std::bad_alloc(); });
}

extern "C" KUNLUN_JSC_INTERNAL kunlun_jsc_status
kunlun_jsc_internal_test_unknown_exception_boundary(void) noexcept
{
    return kunlun::jsc::detail::guard([]() -> kunlun_jsc_status { throw 1; });
}
