#include <sys/mman.h>
#include <android/dlext.h>
#include <dlfcn.h>

#include <lsplt.hpp>

#include <base.hpp>

#include "zygisk.hpp"
#include "module.hpp"
#include "solist.hpp"

using namespace std;

int zygisk_request(int req) {
    ZLOGD("zygisk_request: req=%d\n", req);
    int fd = connect_daemon(RequestCode::ZYGISK);
    ZLOGD("zygisk_request: connect_daemon result fd=%d\n", fd);
    
    if (fd < 0) {
        ZLOGD("zygisk_request: connect_daemon failed\n");
        return fd;
    }
    
    write_int(fd, req);
    ZLOGD("zygisk_request: wrote req=%d to fd=%d\n", req, fd);
    
    return fd;
}

ZygiskModule::ZygiskModule(int id, void *handle, void *entry)
    : id(id), handle(handle), entry{entry}, api{}, mod{nullptr} {
    // Make sure all pointers are null
    memset(&api, 0, sizeof(api));
    api.base.impl = this;
    api.base.registerModule = &ZygiskModule::RegisterModuleImpl;
}

bool ZygiskModule::RegisterModuleImpl(ApiTable *api, long *module) {
    if (api == nullptr || module == nullptr)
        return false;

    long api_version = *module;
    // Unsupported version
    if (api_version > ZYGISK_API_VERSION)
        return false;

    // Set the actual module_abi*
    api->base.impl->mod = { module };

    // Fill in API accordingly with module API version
    if (api_version >= 1) {
        api->v1.hookJniNativeMethods = hookJniNativeMethods;
        api->v1.pltHookRegister = [](auto a, auto b, auto c, auto d) {
            if (g_ctx) g_ctx->plt_hook_register(a, b, c, d);
        };
        api->v1.pltHookExclude = [](auto a, auto b) {
            if (g_ctx) g_ctx->plt_hook_exclude(a, b);
        };
        api->v1.pltHookCommit = []() { return g_ctx && g_ctx->plt_hook_commit(); };
        api->v1.connectCompanion = [](ZygiskModule *m) { return m->connectCompanion(); };
        api->v1.setOption = [](ZygiskModule *m, auto opt) { m->setOption(opt); };
    }
    if (api_version >= 2) {
        api->v2.getModuleDir = [](ZygiskModule *m) { return m->getModuleDir(); };
        api->v2.getFlags = [](auto) { return ZygiskModule::getFlags(); };
    }
    if (api_version >= 4) {
        api->v4.pltHookCommit = lsplt::CommitHook;
        api->v4.pltHookRegister = [](dev_t dev, ino_t inode, const char *symbol, void *fn, void **backup) {
            if (dev == 0 || inode == 0 || symbol == nullptr || fn == nullptr)
                return;
            lsplt::RegisterHook(dev, inode, symbol, fn, backup);
        };
        api->v4.exemptFd = [](int fd) { return g_ctx && g_ctx->exempt_fd(fd); };
    }

    return true;
}

bool ZygiskModule::valid() const {
    ZLOGD("ZygiskModule::valid: checking module id=%d\n", id);
    
    if (mod.api_version == nullptr) {
        ZLOGD("ZygiskModule::valid: api_version is null, return false\n");
        return false;
    }
    
    ZLOGD("ZygiskModule::valid: api_version=%ld\n", *mod.api_version);
    
    switch (*mod.api_version) {
        case 5:
        case 4:
        case 3:
        case 2:
        case 1: {
            bool result = mod.v1->impl && mod.v1->preAppSpecialize && mod.v1->postAppSpecialize &&
                   mod.v1->preServerSpecialize && mod.v1->postServerSpecialize;
            ZLOGD("ZygiskModule::valid: result=%d\n", result);
            return result;
        }
        default:
            ZLOGD("ZygiskModule::valid: unsupported api_version, return false\n");
            return false;
    }
}

int ZygiskModule::connectCompanion() const {
    if (int fd = zygisk_request(+ZygiskRequest::ConnectCompanion); fd >= 0) {
#ifdef __LP64__
        write_any<bool>(fd, true);
#else
        write_any<bool>(fd, false);
#endif
        write_int(fd, id);
        return fd;
    }
    return -1;
}

int ZygiskModule::getModuleDir() const {
    if (owned_fd fd = zygisk_request(+ZygiskRequest::GetModDir); fd >= 0) {
        write_int(fd, id);
        return recv_fd(fd);
    }
    return -1;
}

void ZygiskModule::setOption(zygisk::Option opt) {
    if (g_ctx == nullptr)
        return;
    ZLOGD("setOption: opt=%d, ctx_flags=0x%x\n", opt, g_ctx->flags);
    switch (opt) {
        case zygisk::FORCE_DENYLIST_UNMOUNT:
            g_ctx->flags |= DO_REVERT_UNMOUNT;
            ZLOGD("setOption: FORCE_DENYLIST_UNMOUNT, new flags=0x%x\n", g_ctx->flags);
            break;
        case zygisk::DLCLOSE_MODULE_LIBRARY:
            unload = true;
            break;
    }
}

uint32_t ZygiskModule::getFlags() {
    return g_ctx ? (g_ctx->info_flags & ~PRIVATE_MASK) : 0;
}

void ZygiskModule::tryUnload() const {
    if (unload) dlclose(handle);
}

// -----------------------------------------------------------------

#define call_app(method)               \
switch (*mod.api_version) {            \
case 1:                                \
case 2: {                              \
    AppSpecializeArgs_v1 a(args);      \
    mod.v1->method(mod.v1->impl, &a);  \
    break;                             \
}                                      \
case 3:                                \
case 4:                                \
case 5:                                \
    mod.v1->method(mod.v1->impl, args);\
    break;                             \
}

void ZygiskModule::preAppSpecialize(AppSpecializeArgs_v5 *args) const {
    call_app(preAppSpecialize)
}

void ZygiskModule::postAppSpecialize(const AppSpecializeArgs_v5 *args) const {
    call_app(postAppSpecialize)
}

void ZygiskModule::preServerSpecialize(ServerSpecializeArgs_v1 *args) const {
    mod.v1->preServerSpecialize(mod.v1->impl, args);
}

void ZygiskModule::postServerSpecialize(const ServerSpecializeArgs_v1 *args) const {
    mod.v1->postServerSpecialize(mod.v1->impl, args);
}

// -----------------------------------------------------------------

void ZygiskContext::plt_hook_register(const char *regex, const char *symbol, void *fn, void **backup) {
    if (regex == nullptr || symbol == nullptr || fn == nullptr)
        return;
    regex_t re;
    if (regcomp(&re, regex, REG_NOSUB) != 0)
        return;
    mutex_guard lock(hook_info_lock);
    register_info.emplace_back(RegisterInfo{re, symbol, fn, backup});
}

void ZygiskContext::plt_hook_exclude(const char *regex, const char *symbol) {
    if (!regex) return;
    regex_t re;
    if (regcomp(&re, regex, REG_NOSUB) != 0)
        return;
    mutex_guard lock(hook_info_lock);
    ignore_info.emplace_back(IgnoreInfo{re, symbol ?: ""});
}

void ZygiskContext::plt_hook_process_regex() {
    if (register_info.empty())
        return;
    for (auto &map : lsplt::MapInfo::Scan()) {
        if (map.offset != 0 || !map.is_private || !(map.perms & PROT_READ)) continue;
        for (auto &reg: register_info) {
            if (regexec(&reg.regex, map.path.data(), 0, nullptr, 0) != 0)
                continue;
            bool ignored = false;
            for (auto &ign: ignore_info) {
                if (regexec(&ign.regex, map.path.data(), 0, nullptr, 0) != 0)
                    continue;
                if (ign.symbol.empty() || ign.symbol == reg.symbol) {
                    ignored = true;
                    break;
                }
            }
            if (!ignored) {
                lsplt::RegisterHook(map.dev, map.inode, reg.symbol, reg.callback, reg.backup);
            }
        }
    }
}

bool ZygiskContext::plt_hook_commit() {
    {
        mutex_guard lock(hook_info_lock);
        plt_hook_process_regex();
        for (auto& reg: register_info) {
            regfree(&reg.regex);
        }
        for (auto& ign: ignore_info) {
            regfree(&ign.regex);
        }
        register_info.clear();
        ignore_info.clear();
    }
    return lsplt::CommitHook();
}

// -----------------------------------------------------------------

int ZygiskContext::get_module_info(int uid, rust::Vec<int> &fds) {
    if (int fd = zygisk_request(+ZygiskRequest::GetInfo); fd >= 0) {
        write_int(fd, uid);
        write_string(fd, process);
#ifdef __LP64__
        write_any<bool>(fd, true);
#else
        write_any<bool>(fd, false);
#endif
        xxread(fd, &info_flags, sizeof(info_flags));
        if (zygisk_should_load_module(info_flags)) {
            fds = recv_fds(fd);
        }
        return fd;
    }
    return -1;
}

void ZygiskContext::sanitize_fds() {
    zygisk_close_logd();

    if (!is_child()) {
        return;
    }

    if (can_exempt_fd() && !exempted_fds.empty()) {
        auto update_fd_array = [&](int old_len) -> jintArray {
            jintArray array = env->NewIntArray(static_cast<int>(old_len + exempted_fds.size()));
            if (array == nullptr)
                return nullptr;

            env->SetIntArrayRegion(
                    array, old_len, static_cast<int>(exempted_fds.size()), exempted_fds.data());
            for (int fd : exempted_fds) {
                if (fd >= 0 && fd < allowed_fds.size()) {
                    allowed_fds[fd] = true;
                }
            }
            *args.app->fds_to_ignore = array;
            return array;
        };

        if (jintArray fdsToIgnore = *args.app->fds_to_ignore) {
            int *arr = env->GetIntArrayElements(fdsToIgnore, nullptr);
            int len = env->GetArrayLength(fdsToIgnore);
            for (int i = 0; i < len; ++i) {
                int fd = arr[i];
                if (fd >= 0 && fd < allowed_fds.size()) {
                    allowed_fds[fd] = true;
                }
            }
            if (jintArray newFdList = update_fd_array(len)) {
                env->SetIntArrayRegion(newFdList, 0, len, arr);
            }
            env->ReleaseIntArrayElements(fdsToIgnore, arr, JNI_ABORT);
        } else {
            update_fd_array(0);
        }
    }

    // Close all forbidden fds to prevent crashing
    auto dir = xopen_dir("/proc/self/fd");
    int dfd = dirfd(dir.get());
    for (dirent *entry; (entry = xreaddir(dir.get()));) {
        int fd = parse_int(entry->d_name);
        if ((fd < 0 || fd >= allowed_fds.size() || !allowed_fds[fd]) && fd != dfd) {
            close(fd);
        }
    }
}

bool ZygiskContext::exempt_fd(int fd) {
    if ((flags & POST_SPECIALIZE) || (flags & SKIP_CLOSE_LOG_PIPE))
        return true;
    if (!can_exempt_fd())
        return false;
    exempted_fds.push_back(fd);
    return true;
}

bool ZygiskContext::can_exempt_fd() const {
    return (flags & APP_FORK_AND_SPECIALIZE) && args.app->fds_to_ignore;
}

static int sigmask(int how, int signum) {
    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, signum);
    return sigprocmask(how, &set, nullptr);
}

void ZygiskContext::fork_pre() {
    // Do our own fork before loading any 3rd party code
    // First block SIGCHLD, unblock after original fork is done
    sigmask(SIG_BLOCK, SIGCHLD);
    pid = old_fork();

    if (!is_child())
        return;

    // Record all open fds
    auto dir = xopen_dir("/proc/self/fd");
    for (dirent *entry; (entry = xreaddir(dir.get()));) {
        int fd = parse_int(entry->d_name);
        if (fd < 0 || fd >= allowed_fds.size()) {
            close(fd);
            continue;
        }
        allowed_fds[fd] = true;
    }
    // The dirfd will be closed once out of scope
    allowed_fds[dirfd(dir.get())] = false;
    // logd_fd should be handled separately
    if (int fd = zygisk_get_logd(); fd >= 0) {
        allowed_fds[fd] = false;
    }
}

void ZygiskContext::fork_post() {
    // Unblock SIGCHLD in case the original method didn't
    sigmask(SIG_UNBLOCK, SIGCHLD);
}

void ZygiskContext::run_modules_pre(rust::Vec<int> &fds) {
    for (int i = 0; i < fds.size(); ++i) {
        owned_fd fd = fds[i];
        struct stat s{};
        if (fstat(fd, &s) != 0 || !S_ISREG(s.st_mode)) {
            fds[i] = -1;
            continue;
        }
        android_dlextinfo info {
            .flags = ANDROID_DLEXT_USE_LIBRARY_FD,
            .library_fd = fd,
        };
        if (void *h = android_dlopen_ext("/jit-cache", RTLD_LAZY, &info)) {
            if (void *e = dlsym(h, "zygisk_module_entry")) {
                modules.emplace_back(i, h, e);
            }
        } else if (flags & SERVER_FORK_AND_SPECIALIZE) {
            ZLOGW("Failed to dlopen zygisk module: %s\n", dlerror());
            fds[i] = -1;
        }
    }

    for (auto it = modules.begin(); it != modules.end();) {
        it->onLoad(env);
        if (it->valid()) {
            ++it;
        } else {
            it = modules.erase(it);
        }
    }

    for (auto &m : modules) {
        if (flags & APP_SPECIALIZE) {
            m.preAppSpecialize(args.app);
        } else if (flags & SERVER_FORK_AND_SPECIALIZE) {
            m.preServerSpecialize(args.server);
        }
    }

    // Memory remapping hiding - hide Zygisk module memory mappings from /proc/self/maps
    // Only for non-system apps (UID >= 10000)
    if ((flags & APP_SPECIALIZE) && (args.app->uid % 100000) >= 10000) {
        for (auto &info : lsplt::MapInfo::Scan()) {
            if (strstr(info.path.data(), "/memfd:jit-zygisk-cache") == nullptr &&
                strstr(info.path.data(), "/modules/") == nullptr)
                continue;
            void *addr = reinterpret_cast<void *>(info.start);
            size_t size = info.end - info.start;
            void *copy = mmap(nullptr, size, PROT_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
            if ((info.perms & PROT_READ) == 0) {
                mprotect(addr, size, PROT_READ);
            }
            memcpy(copy, addr, size);
            mremap(copy, size, size, MREMAP_MAYMOVE | MREMAP_FIXED, addr);
            mprotect(addr, size, info.perms);
        }
    }
}

void ZygiskContext::run_modules_post() {
    flags |= POST_SPECIALIZE;
    for (const auto &m : modules) {
        if (flags & APP_SPECIALIZE) {
            m.postAppSpecialize(args.app);
        } else if (flags & SERVER_FORK_AND_SPECIALIZE) {
            m.postServerSpecialize(args.server);
        }
        m.tryUnload();
    }

    // SoList hiding - hide Zygisk modules from detection
    // Only for non-system apps (UID >= 10000)
    if ((flags & APP_SPECIALIZE) && (args.app->uid % 100000) >= 10000) {
        if (SoList::Initialize()) {
            SoList::NullifySoName("/memfd:jit-zygisk-cache");
            SoList::NullifySoName("/modules/");
        }
    }
}

void ZygiskContext::app_specialize_pre() {
    flags |= APP_SPECIALIZE;

    rust::Vec<int> module_fds;
    owned_fd fd = get_module_info(args.app->uid, module_fds);

    ZLOGD("app_specialize_pre: process=%s, uid=%d, info_flags=0x%x\n",
        process ? process : "(null)", args.app->uid, info_flags);

    // Check for sulist mode
    if (info_flags & +ZygiskStateFlags::AllowlistEnforcing) {
        // Sulist mode
        if (info_flags & +ZygiskStateFlags::ProcessOnAllowList) {
            // Process is on allowlist - mount Magisk for it
            ZLOGI("[%s] is on the sulist (allow)\n", process);
            flags |= DO_ALLOW;
            ZLOGD("app_specialize_pre: set DO_ALLOW, new flags=0x%x\n", flags);
        } else {
            // Process is NOT on allowlist - unmount Magisk
            ZLOGI("[%s] is NOT on the sulist (revert)\n", process);
            flags |= DO_REVERT_UNMOUNT;
            ZLOGD("app_specialize_pre: set DO_REVERT_UNMOUNT (sulist), new flags=0x%x\n", flags);
        }
    } else if ((info_flags & UNMOUNT_MASK) == UNMOUNT_MASK) {
        // Normal denylist mode
        ZLOGI("[%s] is on the denylist\n", process);
        flags |= DO_REVERT_UNMOUNT;
        ZLOGD("app_specialize_pre: set DO_REVERT_UNMOUNT (denylist), new flags=0x%x\n", flags);
    } else if (fd >= 0) {
        run_modules_pre(module_fds);
        ZLOGD("app_specialize_pre: modules loaded, fd=%d\n", (int)fd);
    } else {
        ZLOGD("app_specialize_pre: no special action, fd=%d, info_flags=0x%x\n", (int)fd, info_flags);
    }
}

void ZygiskContext::app_specialize_post() {
    run_modules_post();
    if (info_flags & +ZygiskStateFlags::ProcessIsMagiskApp) {
        setenv("ZYGISK_ENABLED", "1", 1);
    }

    // Cleanups
    env->ReleaseStringUTFChars(args.app->nice_name, process);
}

void ZygiskContext::server_specialize_pre() {
    rust::Vec<int> module_fds;
    if (owned_fd fd = get_module_info(1000, module_fds); fd >= 0) {
        if (module_fds.empty()) {
            write_int(fd, 0);
        } else {
            run_modules_pre(module_fds);

            // Find all failed module ids and send it back to magiskd
            vector<int> failed_ids;
            for (int i = 0; i < module_fds.size(); ++i) {
                if (module_fds[i] < 0) {
                    failed_ids.push_back(i);
                }
            }
            write_vector(fd, failed_ids);
        }
    }
}

void ZygiskContext::server_specialize_post() {
    run_modules_post();
}

// -----------------------------------------------------------------

void ZygiskContext::nativeSpecializeAppProcess_pre() {
    process = env->GetStringUTFChars(args.app->nice_name, nullptr);
    ZLOGV("pre  specialize [%s]\n", process);
    // App specialize does not check FD
    flags |= SKIP_CLOSE_LOG_PIPE;
    app_specialize_pre();
}

void ZygiskContext::nativeSpecializeAppProcess_post() {
    ZLOGV("post specialize [%s]\n", process);
    app_specialize_post();
}

void ZygiskContext::nativeForkSystemServer_pre() {
    ZLOGV("pre  forkSystemServer\n");
    flags |= SERVER_FORK_AND_SPECIALIZE;
    process = "system_server";

    fork_pre();
    if (is_child()) {
        server_specialize_pre();
    }
    sanitize_fds();
}

void ZygiskContext::nativeForkSystemServer_post() {
    if (is_child()) {
        ZLOGV("post forkSystemServer\n");
        server_specialize_post();
    }
    fork_post();
}

void ZygiskContext::nativeForkAndSpecialize_pre() {
    process = env->GetStringUTFChars(args.app->nice_name, nullptr);
    ZLOGV("pre  forkAndSpecialize [%s]\n", process);
    flags |= APP_FORK_AND_SPECIALIZE;

    fork_pre();
    if (is_child()) {
        app_specialize_pre();
    }
    sanitize_fds();
}

void ZygiskContext::nativeForkAndSpecialize_post() {
    if (is_child()) {
        ZLOGV("post forkAndSpecialize [%s]\n", process);
        app_specialize_post();
    }
    fork_post();
}
