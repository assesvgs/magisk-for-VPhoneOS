// Minimal C++ runtime stubs for lsplt (replaces full libcxx link)

#include <cstddef>
#include <cstdlib>

void *operator new(size_t size) {
    return malloc(size);
}

void *operator new[](size_t size) {
    return malloc(size);
}

void operator delete(void *ptr) noexcept {
    free(ptr);
}

void operator delete[](void *ptr) noexcept {
    free(ptr);
}

void operator delete(void *ptr, size_t) noexcept {
    free(ptr);
}

void operator delete[](void *ptr, size_t) noexcept {
    free(ptr);
}

extern "C" void __cxa_pure_virtual() {
    while (true) {}
}

extern "C" int __cxa_guard_acquire(int *guard) {
    return *guard == 0;
}

extern "C" void __cxa_guard_release(int *guard) {
    *guard = 1;
}

extern "C" void __cxa_guard_abort(int *guard) {
    (void)guard;
}

extern "C" int __cxa_atexit(void (*)(void *), void *, void *) {
    return 0;
}

void *__dso_handle = nullptr;
