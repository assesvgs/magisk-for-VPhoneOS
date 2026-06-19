#include <sys/mount.h>
#include <android/dlext.h>
#include <unistd.h>
#include <cstring>
#include <dlfcn.h>
#include <poll.h>

#include <base.hpp>
#include <core.hpp>
#include <flags.h>

#include "zygisk.hpp"

using namespace std;

using comp_entry = void(*)(int);
extern "C" void exec_companion_entry(int, comp_entry);

// Sulist support functions
static int clean_ns64 = -1, clean_ns32 __attribute__((unused)) = -1;

int remote_request_sulist() {
    LOGD("remote_request_sulist: start\n");
    if (int fd = zygisk_request(+ZygiskRequest::SulistRootNs); fd >= 0) {
        int res = read_int(fd);
        LOGD("remote_request_sulist: fd=%d, res=%d\n", fd, res);
        close(fd);
        return res;
    }
    LOGD("remote_request_sulist: failed\n");
    return -1;
}

int remote_request_umount() {
    LOGD("remote_request_umount: start\n");
    if (int fd = zygisk_request(+ZygiskRequest::RevertUnmount); fd >= 0) {
        // directly open fd path from magisk proc without recv_fd
        auto ns_path = read_string(fd);
        LOGD("remote_request_umount: ns_path=[%s]\n", ns_path.data());

        // [诊断] 检查 ns_path 是否为空（daemon 未响应时为空）
        if (ns_path.empty()) {
            LOGE("remote_request_umount: ns_path is empty! daemon did not respond\n");
            close(fd);
            return -1;
        }

        auto clean_ns = xopen(ns_path.data(), O_RDONLY);
        LOGD("remote_request_umount: clean_ns_fd=%d\n", clean_ns);

        if (clean_ns > 0) {
#ifdef MAGISK_DEBUG
            // [诊断] 记录 setns 前的 namespace
            char ns_before[128] = {};
            if (ssize_t len = readlink("/proc/self/ns/mnt", ns_before, sizeof(ns_before)-1); len > 0) {
                ns_before[len] = '\0';
                LOGD("remote_request_umount: ns_before_setns=[%s]\n", ns_before);
            }
#endif

            int setns_ret = xsetns(clean_ns, CLONE_NEWNS);

            if (setns_ret != 0) {
                LOGE("remote_request_umount: xsetns failed: %s\n", strerror(errno));
                close(clean_ns);
                close(fd);
                return -1;
            }

#ifdef MAGISK_DEBUG
            // [诊断] 记录 setns 后的 namespace
            char ns_after[128] = {};
            if (ssize_t len = readlink("/proc/self/ns/mnt", ns_after, sizeof(ns_after)-1); len > 0) {
                ns_after[len] = '\0';
                LOGD("remote_request_umount: ns_after_setns=[%s]\n", ns_after);
            }

            // [诊断] 检查 namespace 是否变化
            if (ns_before[0] && ns_after[0] && strcmp(ns_before, ns_after) != 0) {
                LOGD("remote_request_umount: namespace CHANGED [%s] -> [%s]\n", ns_before, ns_after);
            } else if (ns_before[0] && ns_after[0]) {
                LOGD("remote_request_umount: namespace unchanged [%s]\n", ns_before);
            }
#endif
        } else {
            LOGE("remote_request_umount: failed to open ns_path, errno=%d(%s)\n",
                 errno, strerror(errno));
        }

        close(clean_ns);
        close(fd);
        return 0;
    }
    LOGD("remote_request_umount: zygisk_request failed\n");
    return -1;
}

static void zygiskd(int socket) {
    if (getuid() != 0 || fcntl(socket, F_GETFD) < 0)
        exit(-1);

#if defined(__LP64__)
    set_nice_name("zygiskd64");
    LOGI("* Launching zygiskd64\n");
#else
    set_nice_name("zygiskd32");
    LOGI("* Launching zygiskd32\n");
#endif

    // Load modules
    vector<comp_entry> modules;
    {
        auto module_fds = recv_fds(socket);
        for (int fd : module_fds) {
            comp_entry entry = nullptr;
            struct stat s{};
            if (fstat(fd, &s) == 0 && S_ISREG(s.st_mode)) {
                android_dlextinfo info {
                    .flags = ANDROID_DLEXT_USE_LIBRARY_FD,
                    .library_fd = fd,
                };
                if (void *h = android_dlopen_ext("/jit-cache", RTLD_LAZY, &info)) {
                    *(void **) &entry = dlsym(h, "zygisk_companion_entry");
                } else {
                    LOGW("Failed to dlopen zygisk module: %s\n", dlerror());
                }
            }
            modules.push_back(entry);
            close(fd);
        }
    }

    // ack
    write_int(socket, 0);

    // Start accepting requests
    pollfd pfd = { socket, POLLIN, 0 };
    for (;;) {
        poll(&pfd, 1, -1);
        if (pfd.revents && !(pfd.revents & POLLIN)) {
            // Something bad happened in magiskd, terminate zygiskd
            exit(0);
        }
        int client = recv_fd(socket);
        if (client < 0) {
            // Something bad happened in magiskd, terminate zygiskd
            exit(0);
        }
        int module_id = read_int(client);
        if (module_id >= 0 && module_id < modules.size() && modules[module_id]) {
            exec_companion_entry(client, modules[module_id]);
        } else {
            close(client);
        }
    }
}

// Entrypoint where we need to re-exec ourselves
// This should only ever be called internally
int zygisk_main(int argc, char *argv[]) {
    android_logging();
    if (argc == 3 && argv[1] == "companion"sv) {
        zygiskd(parse_int(argv[2]));
    }
    return 0;
}

// Entrypoint of code injection
extern "C" [[maybe_unused]] NativeBridgeCallbacks NativeBridgeItf {
    .version = 2,
    .padding = {},
    .isCompatibleWith = [](auto) {
        zygisk_logging();
        hook_entry();
        ZLOGD("load success\n");
        return false;
    },
};
