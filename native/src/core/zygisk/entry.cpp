#include <libgen.h>
#include <dlfcn.h>
#include <sys/prctl.h>
#include <sys/mount.h>
#include <android/log.h>
#include <android/dlext.h>
#include <sys/types.h>
#include <sys/wait.h>

#include <base.hpp>
#include <consts.hpp>

#include "zygisk.hpp"
#include "zygisk_utils.hpp"
#include "module.hpp"
#include "ptrace_utils.hpp"

using namespace std;

void *self_handle = nullptr;
int system_server_fd = -1;

// Stub: real logic moved to libzygisk_inject.so
void hook_functions() {}

extern "C" [[maybe_unused]] void zygisk_inject_entry(void *handle) {
    TRACELOGW("hook: zygisk_inject_entry pid=%d\n", getpid());
    self_handle = handle;
    zygisk_logging();
    hook_functions();
    ZLOGD("load success\n");
}

// The following code runs in zygote/app process

static inline bool should_load_modules(uint32_t flags) {
    return (flags & PROCESS_IS_MAGISK_APP) != PROCESS_IS_MAGISK_APP;
}

int remote_get_info(int uid, const char *process, uint32_t *flags, vector<int> &fds) {
    if (int fd = zygisk_request(static_cast<int>(ZygiskRequest::GetInfo)); fd >= 0) {
        write_int(fd, uid);
        write_string(fd, process);
        write_int(fd, sizeof(void*) == 8 ? 1 : 0);
        xxread(fd, flags, sizeof(*flags));
        if (should_load_modules(*flags)) {
            { rust::Vec<int32_t> tmp = recv_fds(fd); fds.assign(tmp.begin(), tmp.end()); }
        }
        return fd;
    }
    return -1;
}

int remote_request_sulist() {
    if (int fd = zygisk_request(static_cast<int>(ZygiskRequest::SulistRootNs)); fd >= 0) {
        int res = read_int(fd);
        close(fd);
        return res;
    }
    return -1;
}

int remote_request_umount() {
    if (int fd = zygisk_request(static_cast<int>(ZygiskRequest::RevertUmount)); fd >= 0) {
        // directly open fd path from magisk proc without recv_fd
        auto ns_path = read_string(fd);
        auto clean_ns = xopen(ns_path.data(), O_RDONLY);
        LOGD("denylist: set to clean ns [%s] fd=[%d]\n", ns_path.data(), clean_ns);
        if (clean_ns > 0) xsetns(clean_ns, CLONE_NEWNS);
        close(clean_ns);
        close(fd);
        return 0;
    }
    return -1;
}

// Daemon-side Zygisk request handlers moved to Rust:
//   native/src/core/zygisk/daemon.rs — zygisk_handler()
//   native/src/core/daemon.rs        — dispatches to Rust zygisk_handler

