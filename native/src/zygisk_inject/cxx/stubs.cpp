#include <cstddef>
#include <cstdlib>
#include <cstdint>

void *operator new(size_t size) { return malloc(size); }
void *operator new[](size_t size) { return malloc(size); }
void operator delete(void *ptr) noexcept { free(ptr); }
void operator delete[](void *ptr) noexcept { free(ptr); }
void operator delete(void *ptr, size_t) noexcept { free(ptr); }
void operator delete[](void *ptr, size_t) noexcept { free(ptr); }

extern "C" void __cxa_pure_virtual() { while (true) {} }

extern "C" int __cxa_guard_acquire(int64_t *guard) {
    uint8_t old = __atomic_load_n(reinterpret_cast<uint8_t *>(guard), __ATOMIC_ACQUIRE);
    if (old == 1) return 0; // already initialized
    return 1; // not initialized (caller proceeds)
}

extern "C" void __cxa_guard_release(int64_t *guard) {
    __atomic_store_n(reinterpret_cast<uint8_t *>(guard), 1, __ATOMIC_RELEASE);
}

extern "C" void __cxa_guard_abort(int64_t *guard) {
    (void)guard;
}

extern "C" int __cxa_atexit(void (*)(void *), void *, void *) { return 0; }

