#include <sys/ptrace.h>
#include <unistd.h>
#include <sys/uio.h>
#include <sys/auxv.h>
#include <elf.h>
#include <link.h>
#include <vector>
#include <string>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <cstdlib>
#include <cstdio>
#include <dlfcn.h>
#include <signal.h>
#include <string>
#include <cerrno>
#include <cstring>
#include <fcntl.h>

#include <core.hpp>
#include "zygisk.hpp"
#include "ptrace_utils.hpp"

using namespace std;

#ifndef PLOGE
#define PLOGE(fmt, ...) LOGE("ptrace: " fmt ": %s\n", ##__VA_ARGS__, strerror(errno))
#endif

#include <flags.h>
#if MAGISK_DEBUG
static void trace_log(const char *fmt, ...) {
    static int fd = -1;
    if (fd < 0) {
        fd = open("/cache/zygisk_trace.log", O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
        if (fd < 0) {
            fd = open("/data/local/tmp/zygisk_trace.log", O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0644);
        }
    }
    if (fd < 0) return;
    va_list ap;
    va_start(ap, fmt);
    vdprintf(fd, fmt, ap);
    va_end(ap);
    fsync(fd);
}
#define TRACELOGE(...) trace_log("E " __VA_ARGS__)
#define TRACELOGW(...) trace_log("W " __VA_ARGS__)
#else
#define TRACELOGE(...) ((void)0)
#define TRACELOGW(...) ((void)0)
#endif



static void gen_rand_str(char *buf, size_t len) {
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd >= 0) {
        ssize_t n = read(fd, buf, len);
        if (n > 0) {
            for (size_t i = 0; i < (size_t)n; i++)
                buf[i] = 'a' + (buf[i] & 0xf);
        }
        close(fd);
    }
}



static bool inject_on_main(int pid, const char *lib_path) {
    struct user_regs_struct regs{}, backup{};
    auto map = Scan_proc(std::to_string(pid));
    if (!get_regs(pid, regs)) {
        ZLOGD("inject: get_regs failed\n");
        TRACELOGE("inject: get_regs failed\n");
        return false;
    }
    auto arg = reinterpret_cast<uintptr_t *>(regs.REG_SP);
    ZLOGD("kernel argument %p %s\n", arg, get_addr_mem_region(map, arg).c_str());
    long argc = 0;
    auto argv = reinterpret_cast<char **>(reinterpret_cast<uintptr_t *>(arg) + 1);
    ZLOGD("argv %p\n", argv);
    if (read_proc(pid, arg, &argc, sizeof(argc)) != sizeof(argc)) {
        ZLOGE("failed to read argc\n");
        TRACELOGE("inject: read argc failed\n");
        return false;
    }
    ZLOGD("argc %ld\n", argc);
    auto envp = argv + argc + 1;
    ZLOGD("envp %p\n", envp);
    auto p = envp;
    int envp_limit = 4096;
    while (envp_limit-- > 0) {
        uintptr_t *buf = nullptr;
        if (read_proc(pid, (uintptr_t *) p, &buf, sizeof(buf)) != sizeof(buf))
            break;
        if (buf != nullptr) ++p;
        else break;
    }
    ++p;
    auto auxv = reinterpret_cast<ElfW(auxv_t) *>(p);
    ZLOGD("auxv %p %s\n", auxv, get_addr_mem_region(map, auxv).c_str());
    auto v = auxv;
    void *entry_addr = nullptr;
    void *addr_of_entry_addr = nullptr;
    int auxv_limit = 512;
    while (auxv_limit-- > 0) {
        ElfW(auxv_t) buf = {};
        if (read_proc(pid, (uintptr_t *) v, &buf, sizeof(buf)) != sizeof(buf))
            break;
        if (buf.a_type == AT_ENTRY) {
            entry_addr = reinterpret_cast<void *>(buf.a_un.a_val);
            addr_of_entry_addr = reinterpret_cast<char *>(v) + offsetof(ElfW(auxv_t), a_un);
            ZLOGD("entry address %p %s (v=%p, entry_addr=%p)\n", entry_addr,
                 get_addr_mem_region(map, entry_addr).c_str(), v, addr_of_entry_addr);
            break;
        }
        if (buf.a_type == AT_NULL) break;
        v++;
    }
    if (entry_addr == nullptr) {
        ZLOGE("failed to get entry\n");
        TRACELOGE("inject: failed to get entry\n");
        return false;
    }

    uintptr_t break_addr = (-0x05ec1cff & ~1) | ((uintptr_t) entry_addr & 1);
    if (write_proc(pid, (uintptr_t *) addr_of_entry_addr, &break_addr, sizeof(break_addr)) <= 0) {
        ZLOGD("inject: write break_addr failed\n");
        TRACELOGE("inject: write break_addr failed\n");
        return false;
    }
    ptrace(PTRACE_CONT, pid, 0, 0);
    int status;
    if (!wait_for_trace(pid, &status, __WALL)) {
        ZLOGD("inject: wait_for_trace after breakpoint failed\n");
        TRACELOGE("inject: wait_for_trace after breakpoint failed\n");
        write_proc(pid, (uintptr_t *) addr_of_entry_addr, &entry_addr, sizeof(entry_addr));
        return false;
    }
    if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGSEGV) {
        if (!get_regs(pid, regs)) {
            ZLOGD("inject: get_regs after SIGSEGV failed\n");
            TRACELOGE("inject: get_regs after SIGSEGV failed\n");
            return false;
        }
        if ((regs.REG_IP & ~1) != (break_addr & ~1)) {
            ZLOGE("stopped at unknown addr %p\n", (void *) regs.REG_IP);
            TRACELOGE("inject: stopped at unknown addr %p\n", (void *) regs.REG_IP);
            return false;
        }
        ZLOGD("stopped at entry\n");
        if (write_proc(pid, (uintptr_t *) addr_of_entry_addr, &entry_addr, sizeof(entry_addr)) <= 0) {
            ZLOGD("inject: restore entry_addr failed\n");
            TRACELOGE("inject: restore entry_addr failed\n");
            return false;
        }
        memcpy(&backup, &regs, sizeof(regs));
        map = Scan_proc(std::to_string(pid));
        auto local_map = Scan_proc();
        auto libc_return_addr = find_module_return_addr(map, "libc.so");
        ZLOGD("libc return addr %p\n", libc_return_addr);

        auto dlopen_addr = find_func_addr(local_map, map, "libdl.so", "dlopen");
        if (dlopen_addr == nullptr) {
            ZLOGD("inject: dlopen not found\n");
            TRACELOGE("inject: dlopen not found\n");
            return false;
        }
        std::vector<long> args;
        auto str = push_string(pid, regs, lib_path);
        args.clear();
        args.push_back((long) str);
        args.push_back((long) RTLD_NOW);
        auto remote_handle = remote_call(pid, regs, (uintptr_t) dlopen_addr, (uintptr_t) libc_return_addr, args);
        ZLOGD("remote handle %p\n", (void *) remote_handle);
        if (remote_handle == 0) {
            ZLOGE("handle is null\n");
            TRACELOGE("inject: remote dlopen handle is null\n");
            auto dlerror_addr = find_func_addr(local_map, map, "libdl.so", "dlerror");
            if (dlerror_addr == nullptr) {
                ZLOGE("find dlerror\n");
                TRACELOGE("inject: dlerror addr not found\n");
                return false;
            }
            args.clear();
            auto dlerror_str_addr = remote_call(pid, regs, (uintptr_t) dlerror_addr, (uintptr_t) libc_return_addr, args);
            ZLOGD("dlerror str %p\n", (void*) dlerror_str_addr);
            if (dlerror_str_addr == 0) return false;
            auto strlen_addr = find_func_addr(local_map, map, "libc.so", "strlen");
            if (strlen_addr == nullptr) {
                ZLOGE("find strlen\n");
                return false;
            }
            args.clear();
            args.push_back(dlerror_str_addr);
            auto dlerror_len = remote_call(pid, regs, (uintptr_t) strlen_addr, (uintptr_t) libc_return_addr, args);
            ZLOGD("dlerror len %ld\n", dlerror_len);
            if (dlerror_len <= 0) return false;
            std::string err;
            err.resize(dlerror_len + 1, 0);
            if (read_proc(pid, (uintptr_t*) dlerror_str_addr, err.data(), dlerror_len) != dlerror_len) {
                ZLOGE("failed to read dlerror string\n");
            } else {
                ZLOGE("dlerror info %s\n", err.c_str());
                TRACELOGE("inject: dlerror: %s\n", err.c_str());
            }
            TRACELOGE("inject: remote dlopen failed\n");
            return false;
        }

        auto dlsym_addr = find_func_addr(local_map, map, "libdl.so", "dlsym");
        if (dlsym_addr == nullptr) {
            ZLOGD("inject: dlsym not found\n");
            TRACELOGE("inject: dlsym not found\n");
            return false;
        }
        args.clear();
        str = push_string(pid, regs, "zygisk_inject_entry");
        args.push_back(remote_handle);
        args.push_back((long) str);
        auto injector_entry = remote_call(pid, regs, (uintptr_t) dlsym_addr, (uintptr_t) libc_return_addr, args);
        ZLOGD("injector entry %p\n", (void*) injector_entry);
        if (injector_entry == 0) {
            ZLOGE("injector entry is null\n");
            TRACELOGE("inject: injector entry is null\n");
            return false;
        }

        args.clear();
        args.push_back(remote_handle);
        remote_call(pid, regs, injector_entry, (uintptr_t) libc_return_addr, args);

        backup.REG_IP = (long) entry_addr;
        ZLOGD("invoke entry\n");
        if (!set_regs(pid, backup)) {
            ZLOGD("inject: set_regs failed\n");
            TRACELOGE("inject: set_regs failed\n");
            return false;
        }
        return true;
    } else {
        ZLOGE("stopped by other reason: %s\n", parse_status(status).c_str());
        TRACELOGE("inject: stopped by other reason: %s\n", parse_status(status).c_str());
    }
    return false;
}

#define STOPPED_WITH(sig, event) (WIFSTOPPED(status) && WSTOPSIG(status) == (sig) && (status >> 16) == (event))

bool trace_zygote(int pid, const char *libpath) {
    ZLOGI("start tracing %d\n", pid);
    TRACELOGW("trace: start tracing pid=%d\n", pid);
#define WAIT_OR_DIE \
    if (!wait_for_trace(pid, &status, __WALL)) { \
        ZLOGD("trace: wait_for_trace failed (line %d)\n", __LINE__); \
        TRACELOGE("trace: wait_for_trace failed (line %d)\n", __LINE__); \
        ptrace(PTRACE_DETACH, pid, 0, 0); \
        return false; \
    }
#define CONT_OR_DIE \
    if (ptrace(PTRACE_CONT, pid, 0, 0) == -1) { \
        PLOGE("cont"); \
        TRACELOGE("trace: cont failed (line %d)\n", __LINE__); \
        ptrace(PTRACE_DETACH, pid, 0, 0); \
        return false; \
    }
    int status;
    ZLOGI("tracing %d (tracer %d)\n", pid, getpid());
    if (ptrace(PTRACE_SEIZE, pid, 0, PTRACE_O_EXITKILL) == -1) {
        if (errno == EINVAL) {
            ZLOGW("PTRACE_O_EXITKILL not supported (kernel < 5.3), retry without it\n");
            if (ptrace(PTRACE_SEIZE, pid, 0, 0) == -1) {
                PLOGE("seize");
                TRACELOGE("trace: seize (no EXITKILL) failed: %s\n", strerror(errno));
                return false;
            }
        } else {
            PLOGE("seize");
            TRACELOGE("trace: seize failed: %s\n", strerror(errno));
            return false;
        }
    }
    WAIT_OR_DIE
    if (STOPPED_WITH(SIGSTOP, PTRACE_EVENT_STOP)) {
        char rstr[26] = { 0 };
        ssprintf(rstr, sizeof(rstr), "/dev/");

        do {
            gen_rand_str(rstr + 5, sizeof(rstr) - 6);
        } while (access(rstr, F_OK) == 0);
        close(xopen(rstr, O_RDONLY | O_CREAT | O_CLOEXEC, 0));
        xmount(libpath, rstr, nullptr, MS_BIND, nullptr);
        bool is_injected = inject_on_main(pid, rstr);
        umount2(rstr, MNT_DETACH);
        rm_rf(rstr);

        if (!is_injected) {
            ZLOGE("failed to inject\n");
            TRACELOGE("trace: inject_on_main returned false\n");
            ptrace(PTRACE_DETACH, pid, 0, 0);
            return false;
        }
        ZLOGD("inject done, continue process\n");
        if (kill(pid, SIGCONT)) {
            PLOGE("kill");
            TRACELOGE("trace: kill SIGCONT failed\n");
            ptrace(PTRACE_DETACH, pid, 0, 0);
            return false;
        }
        CONT_OR_DIE
        WAIT_OR_DIE
        if (STOPPED_WITH(SIGTRAP, PTRACE_EVENT_STOP)) {
            CONT_OR_DIE
            WAIT_OR_DIE
            if (STOPPED_WITH(SIGCONT, 0)) {
                ZLOGD("received SIGCONT\n");
                ptrace(PTRACE_DETACH, pid, 0, SIGCONT);
            }
        } else {
            ZLOGE("unknown state %s, not SIGTRAP + EVENT_STOP\n", parse_status(status).c_str());
            TRACELOGE("trace: unknown state after SIGCONT: %s\n", parse_status(status).c_str());
            ptrace(PTRACE_DETACH, pid, 0, 0);
            return false;
        }
    } else {
        ZLOGE("unknown state %s, not SIGSTOP + EVENT_STOP\n", parse_status(status).c_str());
        TRACELOGE("trace: unknown initial state: %s\n", parse_status(status).c_str());
        ptrace(PTRACE_DETACH, pid, 0, 0);
        return false;
    }
    return true;
}
