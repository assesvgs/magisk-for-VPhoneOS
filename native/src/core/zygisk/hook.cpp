#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/resource.h>
#include <unistd.h>
#include <cstring>
#include <dlfcn.h>
#include <unwind.h>
#include <span>

#include <lsplt.hpp>

#include <base.hpp>

#include "zygisk.hpp"
#include "module.hpp"
#include "jni_hooks.hpp"
#include "solist.hpp"
#include "memory.hpp"

using namespace std;

// *********************
// Zygisk Bootstrapping
// *********************
//
// Zygisk's lifecycle is driven by several PLT function hooks in libandroid_runtime, libart, and
// libnative_bridge. As Zygote is starting up, these carefully selected functions will call into
// the respective lifecycle callbacks in Zygisk to drive the progress forward.
//
// The entire bootstrap process is shown in the graph below.
// Arrows represent control flow, and the blocks are sorted chronologically from top to bottom.
//
// libnative_bridge       libandroid_runtime                zygisk                 libart
//
//                            ┌───────┐
//                            │ start │
//                            └───┬─┬─┘
//                                │ │                                         ┌────────────────┐
//                                │ └────────────────────────────────────────►│LoadNativeBridge│
//                                │                                           └───────┬────────┘
// ┌────────────────┐             │                                                   │
// │LoadNativeBridge│◄────────────┼───────────────────────────────────────────────────┘
// └───────┬────┬───┘             │
//         │    │                 │                     ┌───────────────┐
//         │    └─────────────────┼────────────────────►│NativeBridgeItf│
//         │                      │                     └──────┬────────┘
//         │                      │                            │
//         │                      │                            ▼
//         │                      │                        ┌────────┐
//         │                      │                        │hook_plt│
//         ▼                      │                        └────────┘
//     ┌───────┐                  │
//     │dlclose│                  │
//     └───┬───┘                  │
//         │                      │
//         │                      │                 ┌───────────────────────┐
//         └──────────────────────┼────────────────►│post_native_bridge_load│
//                                │                 └───────────────────────┘
//                                ▼
//                    ┌──────────────────────┐
//                    │ strdup("ZygoteInit") │
//                    └───────────┬────┬─────┘
//                                │    │                ┌───────────────┐
//                                │    └───────────────►│hook_zygote_jni│
//                                │                     └───────────────┘       ┌─────────┐
//                                │                                             │         │
//                                └────────────────────────────────────────────►│   JVM   │
//                                                                              │         │
//                                                                              └──┬─┬────┘
//                      ┌───────────────────┐                                      │ │
//                      │nativeXXXSpecialize│◄─────────────────────────────────────┘ │
//                      └─────────────┬─────┘                                        │
//                                    │                 ┌─────────────┐              │
//                                    └────────────────►│ZygiskContext│              │
//                                                      └─────────────┘              ▼
//                                                                         ┌────────────────────┐
//                                                                         │pthread_attr_destroy│
//                                                                         └─────────┬──────────┘
//                                                     ┌────────────────┐            │
//                                                     │restore_plt_hook│◄───────────┘
//                                                     └────────────────┘
//
// Some notes regarding the important functions/symbols during bootstrap:
//
// * NativeBridgeItf: this symbol is the entry point for android::LoadNativeBridge
// * HookContext::hook_plt(): hook functions like |dlclose| and |strdup|
// * dlclose: the final step in android::LoadNativeBridge. In this function, we unwind the call
//   stack to load the real native bridge if necessary, and fetch NativeBridgeRuntimeCallbacks.
// * strdup: called in AndroidRuntime::start before calling ZygoteInit#main(...)
// * HookContext::hook_zygote_jni(): replace the process specialization functions registered
//   with register_jni_procs. This marks the final step of the code injection bootstrap process.
// * pthread_attr_destroy: called whenever the JVM tries to setup threads for itself. We use
//   this method to cleanup and unload Zygisk from the process.

constexpr const char *kZygoteInit = "com.android.internal.os.ZygoteInit";
constexpr const char *kZygote = "com/android/internal/os/Zygote";
constexpr const char *kForkApp = "nativeForkAndSpecialize";
constexpr const char *kSpecializeApp = "nativeSpecializeAppProcess";
constexpr const char *kForkServer = "nativeForkSystemServer";

using JNIMethods = std::span<JNINativeMethod>;
using JNIMethodsDyn = std::pair<unique_ptr<JNINativeMethod[]>, size_t>;

struct HookContext : JniHookDefinitions {

    vector<tuple<dev_t, ino_t, const char *, void **>> plt_backup;
    const NativeBridgeRuntimeCallbacks *runtime_callbacks = nullptr;
    void *self_handle = nullptr;
    bool should_unmap = false;

    void hook_plt();
    void hook_unloader();
    void restore_plt_hook();
    void hook_zygote_jni();
    void restore_zygote_hook(JNIEnv *env);
    void hook_jni_methods(JNIEnv *env, const char *clz, JNIMethods methods) const;
    void post_native_bridge_load(void *handle);

private:
    void register_hook(dev_t dev, ino_t inode, const char *symbol, void *new_func, void **old_func);
    int hook_jni_methods(JNIEnv *env, jclass clazz, JNIMethods methods) const;
    JNIMethodsDyn get_jni_methods(JNIEnv *env, jclass clazz) const;
};

// -----------------------------------------------------------------

// Global contexts:
//
// HookContext lives as long as Zygisk is loaded in memory. It tracks the process's function
// hooking state and bootstraps code injection until we replace the process specialization methods.
//
// ZygiskContext lives during the process specialization process. It implements Zygisk
// features, such as loading modules and customizing process fork/specialization.

ZygiskContext *g_ctx;
static HookContext *g_hook;

static JniHookDefinitions *get_defs() {
    return g_hook;
}

// -----------------------------------------------------------------

#define DCL_HOOK_FUNC(ret, func, ...) \
ret (*old_##func)(__VA_ARGS__);       \
ret new_##func(__VA_ARGS__)

DCL_HOOK_FUNC(static char *, strdup, const char * str) {
    if (strcmp(kZygoteInit, str) == 0) {
        ZLOGD("strdup: ZygoteInit detected, installing JNI hooks\n");
        g_hook->hook_zygote_jni();
    }
    return old_strdup(str);
}

// Skip actual fork and return cached result if applicable
DCL_HOOK_FUNC(int, fork) {
    return (g_ctx && g_ctx->pid >= 0) ? g_ctx->pid : old_fork();
}

// Unmount stuffs in the process's private mount namespace
DCL_HOOK_FUNC(static int, unshare, int flags) {
    int res = old_unshare(flags);
    if (g_ctx && (flags & CLONE_NEWNS) != 0 && res == 0) {
#ifdef MAGISK_DEBUG
        ZLOGD("unshare: CLONE_NEWNS, ctx_flags=0x%x\n", g_ctx->flags);

        // [诊断] 记录第一次 unshare 后的 mount namespace ID
        char ns_before[128] = {};
        if (ssize_t len = readlink("/proc/self/ns/mnt", ns_before, sizeof(ns_before)-1); len > 0) {
            ns_before[len] = '\0';
            ZLOGD("unshare: ns_before=[%s]\n", ns_before);
        }

        // [诊断] 记录 /sdcard 和 /storage/self/primary 状态
        ZLOGD("unshare: /sdcard exists=%d\n", access("/sdcard", F_OK) == 0);
        ZLOGD("unshare: /storage/self/primary exists=%d\n", access("/storage/self/primary", F_OK) == 0);

        // [诊断] 记录分支决策
        ZLOGD("unshare: branch decision: flags=0x%x (DO_ALLOW=%d, ALLOWLIST_ENFORCED=%d, DO_REVERT=%d)\n",
            g_ctx->flags,
            !!(g_ctx->flags & DO_ALLOW),
            !!(g_ctx->flags & ALLOWLIST_ENFORCED),
            !!(g_ctx->flags & DO_REVERT_UNMOUNT));
#endif

        // 核心逻辑：根据标志决定操作
        int ret = (g_ctx->flags & DO_ALLOW)?
                remote_request_sulist() :
        // 关键：检查 ALLOWLIST_ENFORCED 标志
        (!(g_ctx->flags & ALLOWLIST_ENFORCED) && (g_ctx->flags & DO_REVERT_UNMOUNT))?
                remote_request_umount() : 0 /* do nothing */;
        if (ret == -1) ZLOGE("remote request failed\n");

        // 与 kokoro 一致：仅在 sulist 模式下设置 logging_muted
        if (g_ctx->flags & DO_ALLOW) {
            logging_muted = true;
        }

#ifdef MAGISK_DEBUG
        ZLOGD("unshare: ret=%d\n", ret);
        // [诊断] 记录 remote_request 后的状态
        ZLOGD("unshare: /sdcard exists_after_remote=%d\n", access("/sdcard", F_OK) == 0);
        ZLOGD("unshare: /storage/self/primary exists_after_remote=%d\n", access("/storage/self/primary", F_OK) == 0);
#endif

        // clean up mount id hole by unshare mount namespace twice
        old_unshare(CLONE_NEWNS);

#ifdef MAGISK_DEBUG
        // [诊断] 记录第二次 unshare 后的状态
        char ns_after[128] = {};
        if (ssize_t len = readlink("/proc/self/ns/mnt", ns_after, sizeof(ns_after)-1); len > 0) {
            ns_after[len] = '\0';
            ZLOGD("unshare: ns_after=[%s]\n", ns_after);
        }
        ZLOGD("unshare: /sdcard exists_after_2nd_unshare=%d\n", access("/sdcard", F_OK) == 0);
        ZLOGD("unshare: /storage/self/primary exists_after_2nd_unshare=%d\n", access("/storage/self/primary", F_OK) == 0);

        // [诊断] 检查 namespace 是否变化
        if (ns_before[0] && ns_after[0] && strcmp(ns_before, ns_after) != 0) {
            ZLOGD("unshare: WARNING namespace changed! [%s] -> [%s]\n", ns_before, ns_after);
        } else if (ns_before[0] && ns_after[0]) {
            ZLOGD("unshare: namespace unchanged [%s]\n", ns_before);
        }
#endif

        // 恢复 mount_external 状态
        if (g_ctx->flags & RESTORE_MOUNT_EXTERNAL_NONE) {
            g_ctx->args.app->mount_external = 0;
            ZLOGD("unshare: restored mount_external to 0\n");
        }

        // VPhoneOS 健壮恢复逻辑（如果 /sdcard 仍然不存在）
        if (access("/sdcard", F_OK) != 0) {
            if (access("/share", F_OK) == 0) {  // 检测 VPhoneOS
                ZLOGD("unshare: VPhoneOS detected, attempting robust restore\n");

                // 清理符号链接（无论是否损坏，强制删除）
                struct stat st;
                if (lstat("/sdcard", &st) == 0 && S_ISLNK(st.st_mode)) {
                    unlink("/sdcard");
                    ZLOGD("unshare: removed symlink /sdcard (forced)\n");
                }

                // 选择可靠的挂载源（优先 /data/media/0）
                const char* src = nullptr;
                if (access("/data/media/0", F_OK) == 0) {
                    src = "/data/media/0";
                } else if (access("/storage/emulated/0", F_OK) == 0) {
                    src = "/storage/emulated/0";
                } else if (access("/storage/self/primary", F_OK) == 0) {
                    src = "/storage/self/primary";
                }

                if (src) {
                    if (mkdir("/sdcard", 0755) == 0 || errno == EEXIST) {
                        if (mount(src, "/sdcard", nullptr, MS_BIND, nullptr) == 0) {
                            ZLOGD("unshare: bind mount %s -> /sdcard success\n", src);
                            if (mount(nullptr, "/sdcard", nullptr, MS_SHARED, nullptr) != 0) {
                                ZLOGE("unshare: set MS_SHARED failed: %s\n", strerror(errno));
                            }
                        }
                    }
                }
            }
        }

        // Restore errno back to 0
        errno = 0;
    }
    return res;
}

// This is the last moment before the secontext of the process changes
DCL_HOOK_FUNC(static int, selinux_android_setcontext,
              uid_t uid, bool isSystemServer, const char *seinfo, const char *pkgname) {
    // Pre-fetch logd before secontext transition
    zygisk_get_logd();
    return old_selinux_android_setcontext(uid, isSystemServer, seinfo, pkgname);
}

// Close file descriptors to prevent crashing
DCL_HOOK_FUNC(static void, android_log_close) {
    if (g_ctx == nullptr || !(g_ctx->flags & SKIP_CLOSE_LOG_PIPE)) {
        // This happens during forks like nativeForkApp, nativeForkUsap,
        // nativeForkSystemServer, and nativeForkAndSpecialize.
        zygisk_close_logd();
    }
    old_android_log_close();
}

// It should be safe to assume all dlclose's in libnativebridge are for zygisk_loader
DCL_HOOK_FUNC(static int, dlclose, void *handle) {
    if (!g_hook->self_handle) {
        ZLOGV("dlclose zygisk_loader\n");
        g_hook->post_native_bridge_load(handle);
    }
    return 0;
}

// We cannot directly call `dlclose` to unload ourselves, otherwise when `dlclose` returns,
// it will return to our code which has been unmapped, causing segmentation fault.
// Instead, we hook `pthread_attr_destroy` which will be called when VM daemon threads start.
DCL_HOOK_FUNC(static int, pthread_attr_destroy, void *target) {
    int res = old_pthread_attr_destroy((pthread_attr_t *)target);

    // Only perform unloading on the main thread
    if (gettid() != getpid())
        return res;

    ZLOGV("pthread_attr_destroy\n");
    if (g_hook->should_unmap) {
        g_hook->restore_plt_hook();
        if (g_hook->should_unmap) {
            ZLOGV("dlclosing self\n");
            void *self_handle = g_hook->self_handle;
            delete g_hook;

            // Because both `pthread_attr_destroy` and `dlclose` have the same function signature,
            // we can use `musttail` to let the compiler reuse our stack frame and thus
            // `dlclose` will directly return to the caller of `pthread_attr_destroy`.
            [[clang::musttail]] return dlclose(self_handle);
        }
    }

    delete g_hook;
    return res;
}

#undef DCL_HOOK_FUNC

// -----------------------------------------------------------------

static size_t get_fd_max() {
    rlimit r{32768, 32768};
    getrlimit(RLIMIT_NOFILE, &r);
    return r.rlim_max;
}

ZygiskContext::ZygiskContext(JNIEnv *env, void *args) :
    env(env), args{args}, process(nullptr), pid(-1), flags(0), info_flags(0),
    allowed_fds(get_fd_max()), hook_info_lock(PTHREAD_MUTEX_INITIALIZER) { g_ctx = this; }

ZygiskContext::~ZygiskContext() {
    ZLOGD("~ZygiskContext: is_child=%d\n", is_child());
    // This global pointer points to a variable on the stack.
    // Set this to nullptr to prevent leaking local variable.
    // This also disables most plt hooked functions.
    g_ctx = nullptr;

    if (!is_child())
        return;

    zygisk_close_logd();
    android_logging();

    // Strip out all API function pointers
    for (auto &m : modules) {
        m.clearApi();
    }

    // Release JNI hook memory
    jni_hook::memory_block::release();

    // Cleanup
    g_hook->should_unmap = true;
    g_hook->restore_zygote_hook(env);
    g_hook->hook_unloader();
}

// -----------------------------------------------------------------

inline void *unwind_get_region_start(_Unwind_Context *ctx) {
    auto fp = _Unwind_GetRegionStart(ctx);
#if defined(__arm__)
    // On arm32, we need to check if the pc is in thumb mode,
    // if so, we need to set the lowest bit of fp to 1
    auto pc = _Unwind_GetGR(ctx, 15); // r15 is pc
    if (pc & 1) {
        // Thumb mode
        fp |= 1;
    }
#endif
    return reinterpret_cast<void *>(fp);
}

// As we use NativeBridgeRuntimeCallbacks to reload native bridge and to hook jni functions,
// we need to find it by the native bridge's unwind context.
// For abis that use registers to pass arguments, i.e. arm32, arm64, x86_64, the registers are
// caller-saved, and they are not preserved in the unwind context. However, they will be saved
// into the callee-saved registers, so we will search the callee-saved registers for the second
// argument, which is the pointer to NativeBridgeRuntimeCallbacks.
// For x86, whose abi uses stack to pass arguments, we can directly get the pointer to
// NativeBridgeRuntimeCallbacks from the stack.
static const NativeBridgeRuntimeCallbacks* find_runtime_callbacks(struct _Unwind_Context *ctx) {
    // Find the writable memory region of libart.so, where the NativeBridgeRuntimeCallbacks is located.
    auto [start, end] = []()-> tuple<uintptr_t, uintptr_t> {
        for (const auto &map : lsplt::MapInfo::Scan()) {
            if (map.path.ends_with("/libart.so") && map.perms == (PROT_WRITE | PROT_READ)) {
                ZLOGV("libart.so: start=%p, end=%p\n",
                      reinterpret_cast<void *>(map.start), reinterpret_cast<void *>(map.end));
                return {map.start, map.end};
            }
        }
        return {0, 0};
    }();
#if defined(__aarch64__)
    // r19-r28 are callee-saved registers
    for (int i = 19; i <= 28; ++i) {
        auto val = static_cast<uintptr_t>(_Unwind_GetGR(ctx, i));
        ZLOGV("r%d = %p\n", i, reinterpret_cast<void *>(val));
        if (val >= start && val < end)
            return reinterpret_cast<const NativeBridgeRuntimeCallbacks*>(val);
    }
#elif defined(__arm__)
    // r4-r10 are callee-saved registers
    for (int i = 4; i <= 10; ++i) {
        auto val = static_cast<uintptr_t>(_Unwind_GetGR(ctx, i));
        ZLOGV("r%d = %p\n", i, reinterpret_cast<void *>(val));
        if (val >= start && val < end)
            return reinterpret_cast<const NativeBridgeRuntimeCallbacks*>(val);
    }
#elif defined(__i386__)
    // get ebp, which points to the bottom of the stack frame
    auto ebp = static_cast<uintptr_t>(_Unwind_GetGR(ctx, 5));
    // 1 pointer size above ebp is the old ebp
    // 2 pointer sizes above ebp is the return address
    // 3 pointer sizes above ebp is the 2nd arg
    auto val = *reinterpret_cast<uintptr_t *>(ebp + 3 * sizeof(void *));
    ZLOGV("ebp + 3 * ptr_size = %p\n", reinterpret_cast<void *>(val));
    if (val >= start && val < end)
        return reinterpret_cast<const NativeBridgeRuntimeCallbacks*>(val);
#elif defined(__x86_64__)
    // r12-r15 and rbx are callee-saved registers, but the compiler is likely to use them reversely
    for (int i : {3, 15, 14, 13, 12}) {
        auto val = static_cast<uintptr_t>(_Unwind_GetGR(ctx, i));
        ZLOGV("r%d = %p\n", i, reinterpret_cast<void *>(val));
        if (val >= start && val < end)
            return reinterpret_cast<const NativeBridgeRuntimeCallbacks*>(val);
    }
#elif defined(__riscv)
    // x8-x9, x18-x27 callee-saved registers
    for (int i : {8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27}) {
        auto val = static_cast<uintptr_t>(_Unwind_GetGR(ctx, i));
        ZLOGV("x%d = %p\n", i, reinterpret_cast<void *>(val));
        if (val >= start && val < end)
            return reinterpret_cast<const NativeBridgeRuntimeCallbacks*>(val);
    }
#else
#error "Unsupported architecture"
#endif
    return nullptr;
}

#ifdef MAGISK_DEBUG
// [诊断] 通过 /proc 扫描获取进程 PID（安全替代 popen）
static int get_pid_by_name_safe(const char *name) {
    DIR *dir = opendir("/proc");
    if (!dir) return -1;
    
    struct dirent *entry;
    while ((entry = readdir(dir))) {
        if (entry->d_type != DT_DIR) continue;
        int pid = atoi(entry->d_name);
        if (pid <= 0) continue;
        
        char path[64];
        snprintf(path, sizeof(path), "/proc/%d/cmdline", pid);
        int fd = open(path, O_RDONLY | O_CLOEXEC);
        if (fd >= 0) {
            char cmd[256] = {};
            ssize_t n = read(fd, cmd, sizeof(cmd) - 1);
            close(fd);
            if (n > 0 && strstr(cmd, name)) {
                closedir(dir);
                return pid;
            }
        }
    }
    closedir(dir);
    return -1;
}

// [诊断] 记录 sdcard 相关状态（vold/sdcard 进程、sdcardfs 挂载、/sdcard 状态）
static void log_sdcard_diagnostics(const char *stage) {
    // 进程信息
    ZLOGD("%s: pid=%d, ppid=%d, uid=%d\n", stage, getpid(), getppid(), getuid());
    
    // native bridge 属性
    char nb_prop[256] = {};
    __system_property_get("ro.dalvik.vm.native.bridge", nb_prop);
    ZLOGD("%s: ro.dalvik.vm.native.bridge=%s\n", stage, nb_prop);
    
    // mount namespace ID
    char mnt_ns[128] = {};
    if (ssize_t len = readlink("/proc/self/ns/mnt", mnt_ns, sizeof(mnt_ns)-1); len > 0) {
        mnt_ns[len] = '\0';
        ZLOGD("%s: mount_namespace=%s\n", stage, mnt_ns);
    }
    
    // vold 进程状态（使用安全的 /proc 扫描）
    int vold_pid = get_pid_by_name_safe("vold");
    ZLOGD("%s: vold_pid=%d\n", stage, vold_pid);
    
    // sdcard 进程状态（使用安全的 /proc 扫描）
    int sdcard_pid = get_pid_by_name_safe("sdcard");
    ZLOGD("%s: sdcard_pid=%d\n", stage, sdcard_pid);
    
    // /sdcard 状态
    ZLOGD("%s: /sdcard exists=%d\n", stage, access("/sdcard", F_OK) == 0);
    ZLOGD("%s: /storage/self/primary exists=%d\n", stage, access("/storage/self/primary", F_OK) == 0);
}
#endif

// -----------------------------------------------------------------

void HookContext::post_native_bridge_load(void *handle) {
    self_handle = handle;
#ifdef MAGISK_DEBUG
    ZLOGD("post_native_bridge_load: handle=%p\n", handle);
    log_sdcard_diagnostics("post_native_bridge_load");
#endif

    // 原有的 native bridge 重载逻辑（保留不变）
    using method_sig = const bool (*)(const char *, const NativeBridgeRuntimeCallbacks *);
    struct trace_arg {
        method_sig load_native_bridge;
        const NativeBridgeRuntimeCallbacks *callbacks;
    };
    trace_arg arg{};

    // Unwind to find the address of android::LoadNativeBridge and NativeBridgeRuntimeCallbacks
    _Unwind_Backtrace(+[](_Unwind_Context *ctx, void *arg) -> _Unwind_Reason_Code {
        void *fp = unwind_get_region_start(ctx);
        Dl_info info{};
        dladdr(fp, &info);
        ZLOGV("backtrace: %p %s\n", fp, info.dli_fname ?: "???");
        if (info.dli_fname && std::string_view(info.dli_fname).ends_with("/libnativebridge.so")) {
            auto payload = reinterpret_cast<trace_arg *>(arg);
            payload->load_native_bridge = reinterpret_cast<method_sig>(fp);
            payload->callbacks = find_runtime_callbacks(ctx);
            ZLOGV("NativeBridgeRuntimeCallbacks: %p\n", payload->callbacks);
            return _URC_END_OF_STACK;
        }
        return _URC_NO_REASON;
    }, &arg);

    if (!arg.load_native_bridge || !arg.callbacks)
        return;

    // Reload the real native bridge if necessary
    auto nb = get_prop(NBPROP);
    auto len = sizeof(ZYGISKLDR) - 1;
    if (nb.size() > len) {
        arg.load_native_bridge(nb.c_str() + len, arg.callbacks);
    }
    runtime_callbacks = arg.callbacks;
}

// -----------------------------------------------------------------

void HookContext::register_hook(
        dev_t dev, ino_t inode, const char *symbol, void *new_func, void **old_func) {
    if (!lsplt::RegisterHook(dev, inode, symbol, new_func, old_func)) {
        ZLOGE("Failed to register plt_hook \"%s\"\n", symbol);
        return;
    }
    plt_backup.emplace_back(dev, inode, symbol, old_func);
}

#define PLT_HOOK_REGISTER_SYM(DEV, INODE, SYM, NAME) \
    register_hook(DEV, INODE, SYM, \
    reinterpret_cast<void *>(new_##NAME), reinterpret_cast<void **>(&old_##NAME))

#define PLT_HOOK_REGISTER(DEV, INODE, NAME) \
    PLT_HOOK_REGISTER_SYM(DEV, INODE, #NAME, NAME)

void HookContext::hook_plt() {
    ino_t android_runtime_inode = 0;
    dev_t android_runtime_dev = 0;
    ino_t native_bridge_inode = 0;
    dev_t native_bridge_dev = 0;

    for (auto &map : lsplt::MapInfo::Scan()) {
        if (map.path.ends_with("/libandroid_runtime.so")) {
            android_runtime_inode = map.inode;
            android_runtime_dev = map.dev;
        } else if (map.path.ends_with("/libnativebridge.so")) {
            native_bridge_inode = map.inode;
            native_bridge_dev = map.dev;
        }
    }

    PLT_HOOK_REGISTER(native_bridge_dev, native_bridge_inode, dlclose);
    PLT_HOOK_REGISTER(android_runtime_dev, android_runtime_inode, fork);
    PLT_HOOK_REGISTER(android_runtime_dev, android_runtime_inode, unshare);
    PLT_HOOK_REGISTER(android_runtime_dev, android_runtime_inode, selinux_android_setcontext);
    PLT_HOOK_REGISTER(android_runtime_dev, android_runtime_inode, strdup);
    PLT_HOOK_REGISTER_SYM(android_runtime_dev, android_runtime_inode, "__android_log_close", android_log_close);

    if (!lsplt::CommitHook())
        ZLOGE("plt_hook failed\n");

    // Remove unhooked methods
    std::erase_if(plt_backup, [](auto &t) { return *std::get<3>(t) == nullptr; });
}

void HookContext::hook_unloader() {
    ino_t art_inode = 0;
    dev_t art_dev = 0;

    for (auto &map : lsplt::MapInfo::Scan()) {
        if (map.path.ends_with("/libart.so")) {
            art_inode = map.inode;
            art_dev = map.dev;
            break;
        }
    }

    PLT_HOOK_REGISTER(art_dev, art_inode, pthread_attr_destroy);
    if (!lsplt::CommitHook())
        ZLOGE("plt_hook failed\n");
}

void HookContext::restore_plt_hook() {
    // Unhook plt_hook
    for (const auto &[dev, inode, sym, old_func] : plt_backup) {
        if (!lsplt::RegisterHook(dev, inode, sym, *old_func, nullptr)) {
            ZLOGE("Failed to register plt_hook [%s]\n", sym);
            should_unmap = false;
        }
    }
    if (!lsplt::CommitHook()) {
        ZLOGE("Failed to restore plt_hook\n");
        should_unmap = false;
    }
}

// -----------------------------------------------------------------

JNIMethodsDyn HookContext::get_jni_methods(JNIEnv *env, jclass clazz) const {
    size_t total = runtime_callbacks->getNativeMethodCount(env, clazz);
    auto methods = std::make_unique_for_overwrite<JNINativeMethod[]>(total);
    runtime_callbacks->getNativeMethods(env, clazz, methods.get(), total);
    return std::make_pair(std::move(methods), total);
}

static void register_jni_methods(JNIEnv *env, jclass clazz, JNIMethods methods) {
    for (auto &method : methods) {
        // It's useful to allow nullptr function pointer for restoring hook
        if (!method.fnPtr) continue;

        // It's normal that the method is not found
        if (env->RegisterNatives(clazz, &method, 1) == JNI_ERR || env->ExceptionCheck() == JNI_TRUE) {
            env->ExceptionClear();
            method.fnPtr = nullptr;
        }
    }
}

int HookContext::hook_jni_methods(JNIEnv *env, jclass clazz, JNIMethods methods) const {
    // Backup existing methods
    auto o = get_jni_methods(env, clazz);
    const auto old_methods = span(o.first.get(), o.second);

    // WARNING: the signature field returned from getNativeMethods is in a non-standard format.
    // DO NOT TRY TO USE IT. This is the reason why we try to call RegisterNatives on every single
    // provided JNI methods directly to be 100% sure about whether a signature matches or not.

    // Replace methods
    register_jni_methods(env, clazz, methods);

    // Fetch the new set of native methods
    auto n = get_jni_methods(env, clazz);
    const auto new_methods = span(n.first.get(), n.second);

    // Find the old function pointer and return to caller
    int hook_count = 0;
    for (auto &method : methods) {
        if (!method.fnPtr) continue;
        for (const auto &new_method : new_methods) {
            if (new_method.fnPtr == method.fnPtr) {
                for (const auto &old_method : old_methods) {
                    if (strcmp(old_method.name, new_method.name) == 0 &&
                        strcmp(old_method.signature, new_method.signature) == 0) {
                        ZLOGV("replace %s %s %p -> %p\n",
                            method.name, method.signature, old_method.fnPtr, method.fnPtr);
                        method.fnPtr = old_method.fnPtr;
                        ++hook_count;
                        // Break 2 levels of for loop
                        goto next_method;
                    }
                }
            }
        }
        next_method:
    }
    return hook_count;
}


void HookContext::hook_jni_methods(JNIEnv *env, const char *clz, JNIMethods methods) const {
    jclass clazz;
    if (!runtime_callbacks || !env || !clz || !((clazz = env->FindClass(clz)))) {
        ranges::for_each(methods, [](auto &m) { m.fnPtr = nullptr; });
        return;
    }
    hook_jni_methods(env, clazz, methods);
}

void HookContext::hook_zygote_jni() {
    ZLOGD("hook_zygote_jni: start\n");
    using method_sig = jint(*)(JavaVM **, jsize, jsize *);
    auto get_created_vms = reinterpret_cast<method_sig>(
            dlsym(RTLD_DEFAULT, "JNI_GetCreatedJavaVMs"));
    if (!get_created_vms) {
        for (auto &map: lsplt::MapInfo::Scan()) {
            if (!map.path.ends_with("/libnativehelper.so")) continue;
            void *h = dlopen(map.path.data(), RTLD_LAZY);
            if (!h) {
                ZLOGW("Cannot dlopen libnativehelper.so: %s\n", dlerror());
                break;
            }
            get_created_vms = reinterpret_cast<method_sig>(dlsym(h, "JNI_GetCreatedJavaVMs"));
            dlclose(h);
            break;
        }
        if (!get_created_vms) {
            ZLOGW("JNI_GetCreatedJavaVMs not found\n");
            return;
        }
    }

    JavaVM *vm = nullptr;
    jsize num = 0;
    jint res = get_created_vms(&vm, 1, &num);
    if (res != JNI_OK || vm == nullptr) {
        ZLOGW("JavaVM not found\n");
        return;
    }
    JNIEnv *env = nullptr;
    res = vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6);
    if (res != JNI_OK || env == nullptr) {
        ZLOGW("JNIEnv not found\n");
    }

    JNINativeMethod missing_method{};
    bool replaced_fork_app = false;
    bool replaced_specialize_app = false;
    bool replaced_fork_server = false;

    jclass clazz = env->FindClass(kZygote);
    auto [ptr, count] = get_jni_methods(env, clazz);
    for (const auto methods = span(ptr.get(), count); const auto &method : methods) {
        if (strcmp(method.name, kForkApp) == 0) {
            if (hook_jni_methods(env, clazz, fork_app_methods) == 0) {
                missing_method = method;
                break;
            }
            replaced_fork_app = true;
        } else if (strcmp(method.name, kSpecializeApp) == 0) {
            if (hook_jni_methods(env, clazz, specialize_app_methods) == 0) {
                missing_method = method;
                break;
            }
            replaced_specialize_app = true;
        } else if (strcmp(method.name, kForkServer) == 0) {
            if (hook_jni_methods(env, clazz, fork_server_methods) == 0) {
                missing_method = method;
                break;
            }
            replaced_fork_server = true;
        }
    }

    if (missing_method.name != nullptr) {
        ZLOGE("Cannot hook method: %s %s\n", missing_method.name, missing_method.signature);
        // Restore methods that were already replaced
        if (replaced_fork_app) register_jni_methods(env, clazz, fork_app_methods);
        if (replaced_specialize_app) register_jni_methods(env, clazz, specialize_app_methods);
        if (replaced_fork_server) register_jni_methods(env, clazz, fork_server_methods);
        // Clear the method lists just in case
        ranges::for_each(fork_app_methods, [](auto &m) { m.fnPtr = nullptr; });
        ranges::for_each(specialize_app_methods, [](auto &m) { m.fnPtr = nullptr; });
        ranges::for_each(fork_server_methods, [](auto &m) { m.fnPtr = nullptr; });
    }
}

void HookContext::restore_zygote_hook(JNIEnv *env) {
    jclass clazz = env->FindClass(kZygote);
    register_jni_methods(env, clazz, fork_app_methods);
    register_jni_methods(env, clazz, specialize_app_methods);
    register_jni_methods(env, clazz, fork_server_methods);
}

// -----------------------------------------------------------------

void hook_entry() {
    ZLOGD("hook_entry: initializing\n");
#ifdef MAGISK_DEBUG
    log_sdcard_diagnostics("hook_entry");
#endif
    default_new(g_hook);
    g_hook->hook_plt();
    ZLOGD("hook_entry: done\n");
}

void hookJniNativeMethods(JNIEnv *env, const char *clz, JNINativeMethod *methods, int numMethods) {
    g_hook->hook_jni_methods(env, clz, { methods, static_cast<size_t>(numMethods) });
}
