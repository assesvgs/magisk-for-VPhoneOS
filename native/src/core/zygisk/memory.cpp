#include <sys/mman.h>
#include "memory.hpp"

static inline uintptr_t align_to(uintptr_t addr, size_t align) {
    return (addr + align - 1) & ~(align - 1);
}

namespace jni_hook {

// We know our minimum alignment is WORD size (size of pointer)
static constexpr size_t ALIGN = sizeof(long);

// 4MB is more than enough
static constexpr size_t CAPACITY = (1 << 22);

// Thread-safe initialization via C++11 function-local static
static uint8_t *alloc_area() {
    void *p = mmap(nullptr, CAPACITY, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        LOGE("jni_hook: mmap failed: %d\n", errno);
        return nullptr;
    }
    return static_cast<uint8_t *>(p);
}

static std::atomic<uint8_t *> _area = nullptr;
static std::atomic<uint8_t *> _curr = nullptr;

void *memory_block::allocate(size_t sz) {
    uint8_t *area = _area.load(std::memory_order_acquire);
    if (!area) {
        area = alloc_area();
        if (!area) return nullptr;
        _area.store(area, std::memory_order_release);
        _curr.store(area, std::memory_order_release);
    }

    size_t aligned = align_to(sz, ALIGN);
    uint8_t *p = _curr.fetch_add(aligned);
    if (p + aligned > area + CAPACITY) {
        _curr.fetch_sub(aligned);
        LOGE("jni_hook: out of memory\n");
        return nullptr;
    }
    return p;
}

void memory_block::release() {
    uint8_t *area = _area.exchange(nullptr);
    if (area) {
        _curr = nullptr;
        munmap(area, CAPACITY);
    }
}

} // namespace jni_hook
