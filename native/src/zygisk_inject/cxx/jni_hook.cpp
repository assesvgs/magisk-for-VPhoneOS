#include "jni_hook.h"
#include <jni.h>
#include <dlfcn.h>
#include <sys/mman.h>
#include <cstring>

typedef jint (*JNI_GetCreatedJavaVMs_t)(JavaVM **, jsize, jsize *);
typedef jint (*RegisterNatives_t)(JNIEnv *, jclass, const JNINativeMethod *, jint);

static RegisterNatives_t orig_RegisterNatives = nullptr;

// Hook for RegisterNatives: intercept Zygote method registration
static jint hook_RegisterNatives(JNIEnv *env, jclass clazz,
                                  const JNINativeMethod *methods, jint nMethods) {
    // Call original first
    jint ret = orig_RegisterNatives(env, clazz, methods, nMethods);

    // Get class name
    jclass class_class = env->GetObjectClass(clazz);
    jmethodID getName = env->GetMethodID(class_class, "getName", "()Ljava/lang/String;");
    auto name = (jstring)env->CallObjectMethod(clazz, getName);
    const char *name_str = env->GetStringUTFChars(name, nullptr);

    // Check if this is Zygote class
    if (name_str && strstr(name_str, "com.android.internal.os.Zygote") != nullptr) {
        for (int i = 0; i < nMethods; i++) {
            // Store original methods for specialization interception
            // Future: replace function pointers for forkAndSpecialize etc.
            (void)methods[i].name;
        }
    }
    if (name_str) env->ReleaseStringUTFChars(name, name_str);
    return ret;
}

bool zygisk_hook_jni_env() {
    auto JNI_GetCreatedJavaVMs =
        reinterpret_cast<JNI_GetCreatedJavaVMs_t>(
            dlsym(RTLD_DEFAULT, "JNI_GetCreatedJavaVMs"));
    if (!JNI_GetCreatedJavaVMs) return false;

    JavaVM *vm = nullptr;
    jsize count = 0;
    if (JNI_GetCreatedJavaVMs(&vm, 1, &count) != 0 || count == 0 || vm == nullptr)
        return false;

    JNIEnv *env = nullptr;
    if (vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) != 0 || env == nullptr)
        return false;

    auto *old_functions = env->functions;
    if (old_functions == nullptr) return false;

    size_t table_size = sizeof(JNINativeInterface);
    auto *new_functions = static_cast<JNINativeInterface *>(
        mmap(nullptr, table_size, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    if (new_functions == MAP_FAILED) return false;

    std::memcpy(new_functions, old_functions, table_size);

    // Replace RegisterNatives
    orig_RegisterNatives = old_functions->RegisterNatives;
    new_functions->RegisterNatives = hook_RegisterNatives;
    env->functions = new_functions;

    return true;
}
