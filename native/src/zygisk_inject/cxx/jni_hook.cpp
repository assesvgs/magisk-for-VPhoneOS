#include "jni_hook.h"
#include <jni.h>
#include <dlfcn.h>
#include <sys/mman.h>
#include <cstring>

typedef jint (*JNI_GetCreatedJavaVMs_t)(JavaVM **, jsize, jsize *);
typedef jint (*RegisterNatives_t)(JNIEnv *, jclass, const JNINativeMethod *, jint);

static RegisterNatives_t orig_RegisterNatives = nullptr;
static void *orig_forkAndSpecialize = nullptr;
static void *orig_specializeAppProcess = nullptr;

static jint hook_RegisterNatives(JNIEnv *env, jclass clazz,
                                  const JNINativeMethod *methods, jint nMethods) {
    jint ret = orig_RegisterNatives(env, clazz, methods, nMethods);

    jclass class_class = env->GetObjectClass(clazz);
    if (!class_class || env->ExceptionCheck()) return ret;

    jmethodID getName = env->GetMethodID(class_class, "getName", "()Ljava/lang/String;");
    if (!getName || env->ExceptionCheck()) return ret;

    auto name = (jstring)env->CallObjectMethod(clazz, getName);
    if (!name || env->ExceptionCheck()) return ret;

    const char *name_str = env->GetStringUTFChars(name, nullptr);
    if (!name_str) return ret;

    if (strstr(name_str, "com.android.internal.os.Zygote") != nullptr) {
        for (int i = 0; i < nMethods; i++) {
            if (strcmp(methods[i].name, "nativeForkAndSpecialize") == 0) {
                orig_forkAndSpecialize = methods[i].fnPtr;
            } else if (strcmp(methods[i].name, "nativeSpecializeAppProcess") == 0) {
                orig_specializeAppProcess = methods[i].fnPtr;
            }
        }
    }
    env->ReleaseStringUTFChars(name, name_str);
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

    orig_RegisterNatives = old_functions->RegisterNatives;
    new_functions->RegisterNatives = hook_RegisterNatives;
    env->functions = new_functions;

    mprotect(new_functions, table_size, PROT_READ);

    return true;
}
