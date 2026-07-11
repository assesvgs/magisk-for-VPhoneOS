#include "jni_hook.h"
#include <jni.h>
#include <sys/mman.h>
#include <cstring>

bool zygisk_hook_jni_env() {
    JavaVM *vm = nullptr;
    jsize count = 0;
    if (JNI_GetCreatedJavaVMs(&vm, 1, &count) != 0 || count == 0 || vm == nullptr)
        return false;

    JNIEnv *env = nullptr;
    if (vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) != 0 || env == nullptr)
        return false;

    auto *old_functions = env->functions;
    if (old_functions == nullptr)
        return false;

    size_t table_size = sizeof(JNINativeInterface);
    auto *new_functions = static_cast<JNINativeInterface *>(
        mmap(nullptr, table_size, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    if (new_functions == MAP_FAILED)
        return false;

    std::memcpy(new_functions, old_functions, table_size);
    env->functions = new_functions;

    return true;
}
