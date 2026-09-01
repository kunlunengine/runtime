#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>

namespace kunlun::jsc::detail {

// The shim copies Rust input into this independent allocation before publishing
// it to JSC. No Rust borrow or destructor reaches a collector thread. The state
// remains live while release() runs; JSC calls its finalizer exactly once.
class ExternalBytes {
public:
    explicit ExternalBytes(size_t length)
        : bytes_(new uint8_t[length ? length : 1] {})
    {
#ifdef KUNLUN_JSC_TESTING
        ++live_allocations;
#endif
    }

    ~ExternalBytes() { release(); }
    ExternalBytes(const ExternalBytes &) = delete;
    ExternalBytes &operator=(const ExternalBytes &) = delete;

    uint8_t *data() const noexcept { return bytes_.load(); }

    // Multiple cleanup paths can relinquish storage without double deletion.
    // No reader may use data() concurrently with release().
    void release() noexcept
    {
        auto *bytes = bytes_.exchange(nullptr);
#ifdef KUNLUN_JSC_TESTING
        if (bytes)
            --live_allocations;
#endif
        delete[] bytes;
    }
#ifdef KUNLUN_JSC_TESTING
    inline static std::atomic<size_t> live_allocations { 0 };
#endif

private:
    std::atomic<uint8_t *> bytes_;
};

} // namespace kunlun::jsc::detail
