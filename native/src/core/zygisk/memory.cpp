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

// No need to be thread safe as the initial mmap always happens on the main thread
static uint8_t *_area = nullptr;

static std::atomic<uint8_t *> _curr = nullptr;

void *memory_block::allocate(size_t sz) {
    if (!_area) {
        // Memory will not actually be allocated because physical pages are mapped in on-demand
        void *p = mmap(
                nullptr, CAPACITY, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) {
            LOGE("jni_hook: mmap failed: %d\n", errno);
            return nullptr;
        }
        _area = static_cast<uint8_t *>(p);
        _curr = _area;
    }
    return _curr.fetch_add(align_to(sz, ALIGN));
}

void memory_block::release() {
    if (_area)
        munmap(_area, CAPACITY);
}

} // namespace jni_hook
