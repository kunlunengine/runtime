#ifndef KUNLUN_JSC_EXCEPTION_BOUNDARY_HPP
#define KUNLUN_JSC_EXCEPTION_BOUNDARY_HPP

#include "kunlun_jsc.h"

#include <new>

namespace kunlun::jsc::detail {

template <typename Function>
kunlun_jsc_status guard(Function &&function) noexcept
{
    try {
        return function();
    } catch (const std::bad_alloc &) {
        return KUNLUN_JSC_STATUS_OUT_OF_MEMORY;
    } catch (...) {
        return KUNLUN_JSC_STATUS_CPP_EXCEPTION;
    }
}

} // namespace kunlun::jsc::detail

#endif // KUNLUN_JSC_EXCEPTION_BOUNDARY_HPP
