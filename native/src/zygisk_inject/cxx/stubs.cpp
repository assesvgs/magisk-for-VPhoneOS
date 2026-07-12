#include <cstddef>
#include <cstdlib>
#include <cstdint>

void *operator new(size_t size) {
    void *p = malloc(size);
    if (!p) abort();
    return p;
}
void *operator new[](size_t size) {
    void *p = malloc(size);
    if (!p) abort();
    return p;
}
void operator delete(void *ptr) noexcept { free(ptr); }
void operator delete[](void *ptr) noexcept { free(ptr); }
void operator delete(void *ptr, size_t) noexcept { free(ptr); }
void operator delete[](void *ptr, size_t) noexcept { free(ptr); }

extern "C" void __cxa_pure_virtual() { while (true) {} }

extern "C" int __cxa_guard_acquire(int64_t *guard) {
    // Itanium C++ ABI: 0=uninit, 1=init-in-progress, 0x11=done
    uint8_t expected = 0;
    uint8_t desired = 1;
    if (__atomic_compare_exchange_n(
            reinterpret_cast<uint8_t *>(guard), &expected, desired, false,
            __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        return 1; // acquired, caller should initialize
    }
    return 0; // already initialized or in progress
}

extern "C" void __cxa_guard_release(int64_t *guard) {
    uint8_t done = 0x11;
    __atomic_store_n(reinterpret_cast<uint8_t *>(guard), done, __ATOMIC_RELEASE);
}

extern "C" void __cxa_guard_abort(int64_t *guard) {
    uint8_t zero = 0;
    __atomic_store_n(reinterpret_cast<uint8_t *>(guard), zero, __ATOMIC_RELEASE);
}

extern "C" int __cxa_atexit(void (*)(void *), void *, void *) { return 0; }
