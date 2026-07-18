#include <signal.h>
#include <pthread.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <vector>
#include <string>
#include <set>
#include <map>
#include <unordered_map>
#include <string>
#include <cinttypes>
#include <poll.h>
#include <sys/syscall.h>
#include <sys/inotify.h>
#include <errno.h>
#include <dlfcn.h>
#include <link.h>
#include <sys/uio.h>
#include <core.hpp>
#include <cstdio>

#include "deny.hpp"
#include <sys/ptrace.h>

using namespace std;

static long xptrace(int request, pid_t pid, void *addr = nullptr, void *data = nullptr) {
    long ret = ptrace(request, pid, addr, data);
    if (ret < 0)
        PLOGE("ptrace %d", pid);
    return ret;
}

static inline long xptrace(int request, pid_t pid, void *addr, uintptr_t data) {
    return xptrace(request, pid, addr, reinterpret_cast<void *>(data));
}

#define WEVENT(s) (((s) & 0xffff0000) >> 16)

/* -----------------------------------------------------------------
 * 远程内存写入辅助函数：通过 process_vm_writev 或 PTRACE_POKEDATA
 * 将数据写入目标进程的地址空间。
 * ----------------------------------------------------------------- */
static bool remote_write(pid_t pid, uintptr_t addr, const void *buf, size_t len) {
    // 尝试使用 process_vm_writev（更高效）
    struct iovec local_iov = { .iov_base = const_cast<void*>(buf), .iov_len = len };
    struct iovec remote_iov = { .iov_base = (void*)addr, .iov_len = len };
    ssize_t ret = process_vm_writev(pid, &local_iov, 1, &remote_iov, 1, 0);
    if ((size_t)ret == len) return true;

    // fallback: PTRACE_POKEDATA 逐字写入
    size_t words = (len + sizeof(long) - 1) / sizeof(long);
    const long *src = (const long *)buf;
    for (size_t i = 0; i < words; i++) {
        if (ptrace(PTRACE_POKEDATA, pid, (void*)(addr + i * sizeof(long)),
                   (void*)src[i]) != 0) {
            return false;
        }
    }
    return true;
}

// BPF 常量（linux/filter.h 在 NDK 中可能不全）
#ifndef BPF_LD
#define BPF_LD 0x00
#endif
#ifndef BPF_W
#define BPF_W 0x00
#endif
#ifndef BPF_ABS
#define BPF_ABS 0x20
#endif
#ifndef BPF_RET
#define BPF_RET 0x06
#endif
#ifndef BPF_K
#define BPF_K 0x00
#endif
#ifndef SECCOMP_RET_ALLOW
#define SECCOMP_RET_ALLOW 0x7fff0000
#endif
#ifndef SECCOMP_SET_MODE_FILTER
#define SECCOMP_SET_MODE_FILTER 1
#endif

// BPF 指令: return ALLOW（允许所有系统调用）
// struct sock_filter { u16 code; u8  jt; u8  jf; u32 k; };
// = { 0x06, 0, 0, 0x7fff0000 }
#define BPF_ALLOW_ALL { BPF_RET | BPF_K, 0, 0, SECCOMP_RET_ALLOW }

/* -----------------------------------------------------------------
 * remote_bypass_seccomp — 在目标进程中绕过 seccomp 过滤
 *
 * 流程：
 * 1. 在目标进程栈上写入 BPF "allow all" 过滤器
 * 2. 在目标进程栈上写入 struct sock_fprog
 * 3. 调用 seccomp(SECCOMP_SET_MODE_FILTER, 0, &fprog)
 * 4. 清理栈上的临时数据
 *
 * 原理：SECCOMP_SET_MODE_FILTER 可以用新过滤器叠加，
 * 这里安装的 allow-all 过滤器优先级高于原有过滤器。
 *
 * 返回值：0=成功，-1=失败
 * ----------------------------------------------------------------- */
static long remote_bypass_seccomp(pid_t pid) {
    long ret_val = -1;
#if defined(__aarch64__)
    // 1. 保存寄存器并读取 SP
    struct user_regs_struct saved_regs;
    struct iovec iov = { &saved_regs, sizeof(saved_regs) };
    if (ptrace(PTRACE_GETREGSET, pid, NT_PRSTATUS, &iov) != 0) {
        PLOGE("seccomp_bypass: getregs");
        return -1;
    }

    uintptr_t sp = (uintptr_t)saved_regs.sp;
    // 在栈顶下 128 字节处放置数据（避免踩到现有栈帧）
    uintptr_t filter_addr = sp - 96;   // BPF 指令
    uintptr_t fprog_addr  = sp - 80;   // sock_fprog 结构体

    // 2. 构建 BPF "allow all" 指令
    // struct sock_filter { __u16 code; __u8 jt; __u8 jf; __u32 k; };
    struct sock_filter {
        uint16_t code;
        uint8_t jt, jf;
        uint32_t k;
    };
    struct sock_filter filter = BPF_ALLOW_ALL;
    if (!remote_write(pid, filter_addr, &filter, sizeof(filter))) {
        LOGD("seccomp_bypass: write filter failed\n");
        return -1;
    }

    // 3. 构建 struct sock_fprog
    // struct sock_fprog { unsigned short len; struct sock_filter *filter; };
    struct {
        uint16_t len;
        uint16_t pad;
        uint32_t pad2;
        uint64_t filter_ptr;
    } fprog __attribute__((packed));
    fprog.len = 1;
    fprog.filter_ptr = (uint64_t)filter_addr;
    if (!remote_write(pid, fprog_addr, &fprog, sizeof(fprog))) {
        LOGD("seccomp_bypass: write fprog failed\n");
        return -1;
    }

    // 4. 获取远程 syscall 地址
    uintptr_t remote_syscall_addr = 0;
    {
        char path[64], line[512];
        sprintf(path, "/proc/%d/maps", pid);
        FILE *fp = fopen(path, "re");
        if (fp) {
            while (fgets(line, sizeof(line), fp)) {
                uintptr_t start, end;
                char perms[8], name[256] = {};
                if (sscanf(line, "%lx-%lx %7s %*lx %*x:%*x %*lu %255s",
                           &start, &end, perms, name) >= 4 &&
                    strstr(name, "libc.so") && strstr(perms, "x")) {
                    static void *local_syscall = nullptr;
                    static uintptr_t local_libc_base = 0;
                    if (!local_syscall) {
                        local_syscall = dlsym(RTLD_DEFAULT, "syscall");
                        if (!local_syscall) { fclose(fp); return -1; }
                        Dl_info info;
                        dladdr(local_syscall, &info);
                        local_libc_base = (uintptr_t)info.dli_fbase;
                    }
                    remote_syscall_addr =
                        start + ((uintptr_t)local_syscall - local_libc_base);
                    break;
                }
            }
            fclose(fp);
        }
    }
    if (!remote_syscall_addr) return -1;

    // 5. 调用 seccomp(SECCOMP_SET_MODE_FILTER, 0, &fprog)
    // 通过 remote_call 调用 syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, 0, &fprog)
    struct user_regs_struct call_regs;
    memcpy(&call_regs, &saved_regs, sizeof(call_regs));
    call_regs.regs[0] = __NR_seccomp;                    // syscall number
    call_regs.regs[1] = SECCOMP_SET_MODE_FILTER;         // op
    call_regs.regs[2] = 0;                                // flags
    call_regs.regs[3] = (long)fprog_addr;                 // args (sock_fprog *)
    call_regs.regs[30] = 0x1;                             // LR = SIGSEGV
    call_regs.pc = remote_syscall_addr;

    iov.iov_base = &call_regs;
    if (ptrace(PTRACE_SETREGSET, pid, NT_PRSTATUS, &iov) != 0) {
        PLOGE("seccomp_bypass: setregs");
        goto restore_seccomp;
    }

    if (ptrace(PTRACE_CONT, pid, 0, 0) != 0) {
        PLOGE("seccomp_bypass: cont");
        goto restore_seccomp;
    }

    int status;
    waitpid(pid, &status, __WALL);
    if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGSEGV) {
        iov.iov_base = &call_regs;
        if (ptrace(PTRACE_GETREGSET, pid, NT_PRSTATUS, &iov) == 0) {
            ret_val = (long)call_regs.regs[0];
        }
    } else if (WIFSTOPPED(status)) {
        LOGW("seccomp_bypass: pid=%d unexpected sig=%d\n", pid, WSTOPSIG(status));
    }

restore_seccomp:
    iov.iov_base = &saved_regs;
    ptrace(PTRACE_SETREGSET, pid, NT_PRSTATUS, &iov);
    return ret_val;

#else
    (void)pid;
    return -1;
#endif
}

/* -----------------------------------------------------------------
 * remote_call_func — 在目标进程中通过 ptrace 执行远程函数调用
 *
 * 原理：找到远程进程 libc 中 target_fn 的地址，
 * 通过 ptrace 设置寄存器让目标进程执行 target_fn(args...)。
 *
 * 执行流程：
 * 1. 保存目标寄存器
 * 2. 设置 x0-x5 = 参数, x30(LR) = 1(无效→SIGSEGV→被我们捕获)
 * 3. 设置 PC = 远程函数地址
 * 4. PTRACE_CONT 单步执行
 * 5. waitpid 捕获 SIGSEGV（函数返回时因 LR=1 触发）
 * 6. 读 x0 = 返回值
 * 7. 恢复原始寄存器
 *
 * 返回值：远程函数的返回值
 *        失败返回 -1
 * ----------------------------------------------------------------- */
static long remote_call_func(pid_t pid, uintptr_t fn_addr,
                              long arg1, long arg2, long arg3, long arg4) {
    long ret_val = -1;
#if defined(__aarch64__)
    struct user_regs_struct old_regs, new_regs;
    struct iovec iov = { &old_regs, sizeof(old_regs) };

    if (ptrace(PTRACE_GETREGSET, pid, NT_PRSTATUS, &iov) != 0) {
        PLOGE("remote_call: getregs");
        return -1;
    }

    memcpy(&new_regs, &old_regs, sizeof(new_regs));
    // ARM64 函数调用约定: x0-x5 = 参数, x30 = LR, PC = 函数地址
    new_regs.regs[0] = arg1;
    new_regs.regs[1] = arg2;
    new_regs.regs[2] = arg3;
    new_regs.regs[3] = arg4;
    new_regs.regs[30] = 0x1;      // LR = 无效地址 → 返回时 SIGSEGV
    new_regs.pc = fn_addr;

    iov.iov_base = &new_regs;
    if (ptrace(PTRACE_SETREGSET, pid, NT_PRSTATUS, &iov) != 0) {
        PLOGE("remote_call: setregs");
        return -1;
    }

    if (ptrace(PTRACE_CONT, pid, 0, 0) != 0) {
        PLOGE("remote_call: cont");
        goto restore;
    }

    int status;
    waitpid(pid, &status, __WALL);
    if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGSEGV) {
        iov.iov_base = &new_regs;
        if (ptrace(PTRACE_GETREGSET, pid, NT_PRSTATUS, &iov) == 0) {
            ret_val = (long)new_regs.regs[0];
        }
    } else if (WIFSTOPPED(status)) {
        LOGW("remote_call: pid=%d unexpected sig=%d\n", pid, WSTOPSIG(status));
    } else {
        LOGW("remote_call: pid=%d unexpected status=%x\n", pid, status);
    }

restore:
    iov.iov_base = &old_regs;
    ptrace(PTRACE_SETREGSET, pid, NT_PRSTATUS, &iov);
    return ret_val;

#else
    (void)pid; (void)fn_addr; (void)arg1; (void)arg2; (void)arg3; (void)arg4;
    return -1;
#endif
}

/* -----------------------------------------------------------------
 * remote_syscall — 在目标进程中执行系统调用（通过 libc syscall() 函数）
 *
 * 在 libc 中找到 syscall 函数的远程地址，然后通过 remote_call_func 调用。
 * 用于在 denylist 目标进程中执行 seccomp 操作。
 *
 * 参数: pid - 目标进程 PID
 *       sysno - 系统调用号 (__NR_xxx)
 *       arg1-arg3 - 系统调用参数
 *
 * 返回值: 系统调用的返回值，失败返回 -1
 * ----------------------------------------------------------------- */
static long remote_syscall(pid_t pid, long sysno, long arg1, long arg2, long arg3) {
    // 扫描远程进程 /proc/pid/maps 找到 libc 基址（可执行段）
    char path[64], line[512];
    uintptr_t remote_libc_base = 0;
    sprintf(path, "/proc/%d/maps", pid);

    FILE *fp = fopen(path, "re");
    if (!fp) return -1;
    while (fgets(line, sizeof(line), fp)) {
        uintptr_t start, end;
        char perms[8], name[256] = {};
        if (sscanf(line, "%lx-%lx %7s %*lx %*x:%*x %*lu %255s",
                   &start, &end, perms, name) >= 4 &&
            strstr(name, "libc.so") && strstr(perms, "x")) {
            remote_libc_base = start;
            break;
        }
    }
    fclose(fp);
    if (!remote_libc_base) return -1;

    // 通过 dlsym + dladdr 计算本地 libc 中 syscall 的偏移
    static void *local_syscall = nullptr;
    static uintptr_t local_libc_base = 0;
    if (!local_syscall) {
        local_syscall = dlsym(RTLD_DEFAULT, "syscall");
        if (!local_syscall) return -1;
        Dl_info info;
        if (!dladdr(local_syscall, &info) || !info.dli_fbase)
            return -1;
        local_libc_base = (uintptr_t)info.dli_fbase;
    }

    uintptr_t remote_syscall_addr =
        remote_libc_base + ((uintptr_t)local_syscall - local_libc_base);

    // syscall() 的函数签名：long syscall(long number, ...);
    // ARM64 bionic 实现：mov x8, x0; mov x0, x1; mov x1, x2; ... svc #0; ret
    // 所以参数移位：x0=sysno(→x8), x1=arg1(→x0), x2=arg2(→x1), x3=arg3(→x2)
    return remote_call_func(pid, remote_syscall_addr, sysno, arg1, arg2, arg3);
}

// Process monitoring
pthread_t monitor_thread;
static int inotify_fd = -1;
static int data_system_wd = -1;
static volatile sig_atomic_t check_zygote_pending = 0;

static int parse_ppid(int pid);
static bool check_process(int pid, const char *process = 0, const char *context = 0, const char *exe = 0);
static bool is_process(int pid, int uid = 0);
static void new_zygote(int pid);

/******************
 * Data structures
 ******************/

// zygote pid -> mnt ns
static map<int, struct stat> zygote_map;

// PID tracking sets (using hashmap to avoid stack-allocated bitset bounds)
static unordered_map<int, bool> attaches;
static unordered_map<int, bool> allowed;
static unordered_map<int, bool> checked;

/********
 * Utils
 ********/

// #define PTRACE_LOG(fmt, args...) LOGD("PID=[%d] " fmt, pid, ##args)
#define PTRACE_LOG(...)

static void detach_pid(int pid, int signal = 0) {
    attaches[pid] = false;
    allowed[pid] = false;
    checked[pid] = false;
    ptrace(PTRACE_DETACH, pid, 0, signal);
    PTRACE_LOG("detach\n");
}

static inline int read_ns(const int pid, struct stat *st) {
    char path[32];
    sprintf(path, "/proc/%d/ns/mnt", pid);
    return stat(path, st);
}

static int parse_ppid(int pid) {
    char path[32];
    int ppid;
    sprintf(path, "/proc/%d/stat", pid);
    auto stat = open_file(path, "re");
    if (!stat)
        return -1;
    // PID COMM STATE PPID .....
    fscanf(stat.get(), "%*d %*s %*c %d", &ppid);

    return ppid;
}

static bool is_zygote_done() {
#ifdef __LP64__
    int zygote_count = (HAVE_32)? 2:1;
    if (zygote_map.size() >= zygote_count)
        return true;
#else
    if (zygote_map.size() >= 1)
        return true;
#endif

    return false;
}

static inline bool read_file(const char *file, char *buf, int count){
    FILE *fp = fopen(file, "re");
    if (!fp) return false;
    bool ok = (fread(buf, count, 1, fp) == 1);
    fclose(fp);
    if (!ok) {
        buf[0] = '\0';
    }
    return ok;
}

static bool check_process(int pid, const char *process, const char *context, const char *exe) {
    char path[128];
    char buf[1024];
    ssize_t len;

    if (!process) goto check_context;
    sprintf(path, "/proc/%d/cmdline", pid);
    if (!read_file(path,buf,sizeof(buf)) ||
        strcmp(buf, process) != 0)
        return false;

    check_context:
    if (!context) goto check_exe;
    sprintf(path, "/proc/%d/attr/current", pid);
    if (!read_file(path,buf,sizeof(buf)) || 
        !strstr(buf, context))
        return false;

    check_exe:
    if (!exe) goto final;
    sprintf(path, "/proc/%d/exe", pid);
    len = readlink(path, buf, sizeof(buf)-1);
    if (len != -1) {
      buf[len] = '\0';
    }
    if (strcmp(buf, exe) != 0)
        return false;

    final:
    return true;
}

static bool is_zygote(int pid){
    return check_process(pid, "zygote", "u:r:zygote:s0", nullptr)  
        || check_process(pid, "zygote64", "u:r:zygote:s0", nullptr)
        || check_process(pid, "zygote32", "u:r:zygote:s0", nullptr);
}

static void check_zygote(){
    if (su_bin_fd < 0) return;

    bool system_server_started = false;
    vector<int> zygote_list;

    crawl_procfs([&zygote_list, &system_server_started](int pid) -> bool {
        // Zygote process
        if (is_process(pid) && is_zygote(pid) && parse_ppid(pid) == 1) {
            zygote_list.push_back(pid);
            return true;
        }

        // system_server: pid == 1000 and zygote is ppid
        if (is_process(pid, 1000) && is_zygote(parse_ppid(pid))) {
            system_server_started = true;
            return true;
        }

        // Others
        return true;
    });

    if (system_server_started) {
        // system_server, starting trace zygote
        for (int i = 0; i < zygote_list.size(); i++) {
            new_zygote(zygote_list[i]);
        }
    }

    if (is_zygote_done()) {
        // Stop periodic scanning
        timeval val { .tv_sec = 0, .tv_usec = 0 };
        itimerval interval { .it_interval = val, .it_value = val };
        setitimer(ITIMER_REAL, &interval, nullptr);
    }
}

void umount_all_zygote() {
    crawl_procfs([](int pid) -> bool {
        // Unmount all Magisk from zygote process by default
        if (is_process(pid) && is_zygote(pid) && parse_ppid(pid) == 1) {
            revert_daemon(pid, -2);
        }
        return true;
    });
}       

#define APP_PROC "/system/bin/app_process"

static void setup_inotify() {
    inotify_fd = inotify_init1(IN_CLOEXEC);
    if (inotify_fd < 0)
        return;

    // Setup inotify asynchronous I/O
    fcntl(inotify_fd, F_SETFL, O_ASYNC);
    struct f_owner_ex ex = {
        .type = F_OWNER_TID,
        .pid = gettid()
    };
    fcntl(inotify_fd, F_SETOWN_EX, &ex);

    // Monitor packages.xml
    data_system_wd = inotify_add_watch(inotify_fd, "/data/system", IN_CLOSE_WRITE);

    // Monitor app installation
    inotify_add_watch(inotify_fd, APP_DATA_DIR, IN_CREATE);
    DIR *dirfp = opendir(APP_DATA_DIR);
    if (dirfp) {
           char buf[4098];
        struct dirent *dp;
        while ((dp = readdir(dirfp)) != nullptr) {
            ssprintf(buf, sizeof(buf) - 1, "%s/%s", APP_DATA_DIR, dp->d_name);
            if (strcmp(dp->d_name, ".") == 0 || strcmp(dp->d_name, "..") == 0)
                continue;
            LOGD("proc_monitor: monitor userspace ID=[%s]\n", dp->d_name);
            inotify_add_watch(inotify_fd, buf, IN_ATTRIB);
        }
        closedir(dirfp);
    }

    // Monitor app_process
    if (access(APP_PROC "32", F_OK) == 0) {
        inotify_add_watch(inotify_fd, APP_PROC "32", IN_ACCESS);
        if (access(APP_PROC "64", F_OK) == 0)
            inotify_add_watch(inotify_fd, APP_PROC "64", IN_ACCESS);
    } else {
        inotify_add_watch(inotify_fd, APP_PROC, IN_ACCESS);
    }
}

static bool is_process(int pid, int uid) {
    char buf[128];
    char key[32];
    int tgid;
    struct stat st{};
    sprintf(buf, "/proc/%d", pid);
    if (stat(buf, &st) || st.st_uid != uid)
        return false;
    sprintf(buf, "/proc/%d/status", pid);
    auto fp = fopen(buf, "re");
    // PID is dead
    if (!fp)
        return false;
    while (fgets(buf, sizeof(buf), fp)) {
        sscanf(buf, "%s", key);
        if (key == "Tgid:"sv) {
            sscanf(buf, "%*s %d", &tgid);
            fclose(fp);
            return tgid == pid;
        }
    }
    fclose(fp);
    return false;
}

/************************
 * Async signal handlers
 ************************/

#define USAP_ENABLED "persist.device_config.runtime_native.usap_pool_enabled" 

// Make sure we can actually read stuffs 
// or else the whole thread will be blocked. 
#define POLL_EVENT \
    struct pollfd pfd = { \
        .fd = inotify_fd, \
        .events = POLLIN, \
        .revents = 0 \
    }; \
    if (poll(&pfd, 1, 0) <= 0) \
        return;

#define PROCESS_EVENT \
    do { \
        char buf[512]; \
        auto event = reinterpret_cast<struct inotify_event *>(buf); \
        read(inotify_fd, buf, sizeof(buf)); \
        if (event->mask & IN_CREATE) { \
            std::string path = std::string(APP_DATA_DIR) + "/" + event->name; \
            LOGD("proc_monitor: monitor userspace ID=[%s]\n", event->name); \
            inotify_add_watch(inotify_fd, path.data(), IN_ATTRIB); \
            break; \
        } \
        if ((event->wd == data_system_wd && event->name == "packages.xml"sv) || (event->mask & IN_ATTRIB)) \
            rescan_apps(); \
        check_zygote(); \
    } while (false);



static void inotify_event(int) {
    POLL_EVENT
    PROCESS_EVENT
}

static void term_thread(int) {
    LOGD("proc_monitor: cleaning up\n");
    zygote_map.clear();
    attaches.clear();
    checked.clear();
    allowed.clear();
    int old_fd = inotify_fd;
    inotify_fd = -1;
    close(old_fd);
    monitor_thread = -1;
    // Restore all signal handlers that was set
    sigset_t set;
    sigfillset(&set);
    pthread_sigmask(SIG_BLOCK, &set, nullptr);
    struct sigaction act{};
    act.sa_handler = SIG_DFL;
    sigaction(SIGTERMTHRD, &act, nullptr);
    sigaction(SIGIO, &act, nullptr);
    sigaction(SIGALRM, &act, nullptr);
    LOGD("proc_monitor: terminate\n");
    pthread_exit(nullptr);
}

static bool ino_equal(struct stat st, struct stat st2){
    return st.st_dev == st2.st_dev &&
        st.st_ino == st2.st_ino;
}

static int check_pid(int pid) {
    char path[128];
    char cmdline[1024];
    int uid = -1;
    int ppid = -1;
    struct stat st;
    sprintf(path, "/proc/%d", pid);
    if (stat(path, &st)) {
        // Process died unexpectedly, ignore
        goto not_target;
    }
    uid = st.st_uid;
    if (uid == 0) {
        return 0;
    }

    // check cmdline
    ssprintf(path, sizeof(path), "/proc/%d/cmdline", pid);
    if (!read_file(path, cmdline, sizeof(cmdline)))
        // Process died unexpectedly, ignore
        goto not_target;

    // still zygote
    if (cmdline == "zygote"sv || cmdline == "zygote32"sv || cmdline == "zygote64"sv ||
        cmdline == "usap32"sv || cmdline == "usap64"sv || cmdline == "<pre-initialized>"sv)
        return 0;

    if (!is_deny_target(uid, cmdline, 95)) {
        goto not_target;
    }

    // Ensure ns is separated
    {
        struct stat ppid_st;
        ppid = parse_ppid(pid);
        read_ns(pid, &st);
        read_ns(ppid, &ppid_st);
        if (ino_equal(st, ppid_st)) {
            LOGW("proc_monitor: skip [%s] PID=[%d] PPID=[%d] UID=[%d]\n", cmdline, pid, ppid, uid);
            goto not_target;
        }
    }

    LOGI("proc_monitor: [%s] PID=[%d] UID=[%d]\n", cmdline, pid, uid);
    // Check PID liveness before detach: kill(pid, 0) works even on ptrace-stopped processes.
    // This avoids signaling a recycled PID between detach and SIGSTOP.
    if (kill(pid, 0) == -1 && errno == ESRCH) return 0;
    detach_pid(pid);
    kill(pid, SIGSTOP);

    {
        LOGD("proc_monitor: sulist_enabled=%d, PID=[%d]\n", sulist_enabled, pid);
        (sulist_enabled) ?
        // if sulist is enabled
        // the target is the process we want to mount magisk
        // else, the target is the process we want to unmount magisk
        mount_magisk_to_pid(pid) : revert_daemon(pid, -2);
    }

not_target:
    detach_pid(pid);
    return 1;
}

static void new_zygote(int pid) {
    struct stat st, init_st;
    if (read_ns(pid, &st) || read_ns(1, &init_st) || 
        (init_st.st_ino == st.st_ino && init_st.st_dev == st.st_dev))
        return;

    auto it = zygote_map.find(pid);
    if (it != zygote_map.end()) {
        it->second = st;
        return;
    }

    // check if pid is attached
    if (zygote_map.count(pid))
        return;

    LOGI("proc_monitor: zygote PID=[%d]\n", pid);

    // attach_zygote
    if (xptrace(PTRACE_ATTACH, pid) == -1)
        return;
    LOGI("proc_monitor: ptrace zygote PID=[%d]\n", pid);
    zygote_map[pid] = st;

    if (sulist_enabled) revert_daemon(pid, -2);

    waitpid(pid, nullptr, __WALL | __WNOTHREAD);
    xptrace(PTRACE_SETOPTIONS, pid, nullptr,
            PTRACE_O_TRACEFORK | PTRACE_O_TRACEVFORK | PTRACE_O_TRACEEXIT);
    xptrace(PTRACE_CONT, pid);
}

#define DETACH_AND_CONT { detach_pid(pid); continue; }

static std::string get_content(int pid, const char *file) {
    char buf[1024];
    sprintf(buf, "/proc/%d/%s", pid, file);
    FILE *fp = fopen(buf, "re");
    if (fp) {
        fgets(buf, sizeof(buf), fp);
        fclose(fp);
        return std::string(buf);
    }
    return std::string("");
}

void proc_monitor() {
    // Prevent duplicate start
    if (monitor_thread != -1 && monitor_thread != pthread_self())
        return;
    monitor_thread = pthread_self();

    // Reset cached result
    zygote_map.clear();
    attaches.clear();
    checked.clear();
    allowed.clear();

    // Backup original mask
    sigset_t orig_mask;
    pthread_sigmask(SIG_SETMASK, nullptr, & orig_mask);

    sigset_t unblock_set;
    sigemptyset( & unblock_set);
    sigaddset( & unblock_set, SIGTERMTHRD);
    sigaddset( & unblock_set, SIGIO);
    sigaddset( & unblock_set, SIGALRM);

    struct sigaction act {};
    sigfillset( & act.sa_mask);
    act.sa_handler = SIG_IGN;
    sigaction(SIGTERMTHRD, & act, nullptr);
    sigaction(SIGIO, & act, nullptr);
    sigaction(SIGALRM, & act, nullptr);

    // Temporary unblock to clear pending signals
    pthread_sigmask(SIG_UNBLOCK, & unblock_set, nullptr);
    pthread_sigmask(SIG_SETMASK, & orig_mask, nullptr);

    act.sa_handler = term_thread;
    sigaction(SIGTERMTHRD, & act, nullptr);
    act.sa_handler = inotify_event;
    sigaction(SIGIO, & act, nullptr);
    check_zygote_pending = 0;
    act.sa_handler = [](int) {
        check_zygote_pending = 1;
    };
    sigaction(SIGALRM, & act, nullptr);

    setup_inotify();

    // First try find existing system server and zygote
    check_zygote();
    rescan_apps();
    if (!is_zygote_done()) {
        // Periodic scan every 250ms
        timeval val {
            .tv_sec = 0, .tv_usec = 250000
        };
        itimerval interval {
            .it_interval = val, .it_value = val
        };
        setitimer(ITIMER_REAL, & interval, nullptr);
    }

    for (int status;;) {
        pthread_sigmask(SIG_UNBLOCK, & unblock_set, nullptr);
        const int pid = waitpid(-1, & status, __WALL | __WNOTHREAD);
        if (pid < 0) {
            if (errno == ECHILD) {
                // Check if a periodic scan was requested via signal
                if (check_zygote_pending) {
                    check_zygote_pending = 0;
                    check_zygote();
                }
                // Nothing to wait yet, sleep and wait till signal interruption
                LOGD("proc_monitor: nothing to monitor, wait for signal\n");
                struct timespec ts = {
                    .tv_sec = INT_MAX,
                    .tv_nsec = 0
                };
                nanosleep( & ts, nullptr);
            }
            continue;
        }

        pthread_sigmask(SIG_SETMASK, & orig_mask, nullptr);

        if (check_zygote_pending) {
            check_zygote_pending = 0;
            check_zygote();
        }

        if (!WIFSTOPPED(status) /* Ignore if not ptrace-stop */ )
            DETACH_AND_CONT;

        int event = WEVENT(status);
        int signal = WSTOPSIG(status);

        if (signal == SIGTRAP && zygote_map.count(pid) & event) {
            unsigned long msg;
            xptrace(PTRACE_GETEVENTMSG, pid, nullptr, & msg);
            switch (event) {
            case PTRACE_EVENT_FORK:
            case PTRACE_EVENT_VFORK:
                PTRACE_LOG("zygote forked: [%lu]\n", msg);
                attaches[msg] = true;
                break;
            case PTRACE_EVENT_EXIT:
                PTRACE_LOG("zygote exited with status: [%lu]\n", msg);
                [[fallthrough]];
            default:
                zygote_map.erase(pid);
                DETACH_AND_CONT;
            }
            xptrace(PTRACE_CONT, pid);
        } else if (signal == (SIGTRAP | 0x80)) {
            do {
                struct stat st {};
                char path[128];
                if (checked[pid]) goto CHECK_PROC;
                sprintf(path, "/proc/%d", pid);
                stat(path, & st);
                PTRACE_LOG("UID=[%d]\n", st.st_uid);
                if (st.st_uid == 0)
                    continue;
                //LOGD("proc_monitor: PID=[%d] UID=[%d]\n", pid, st.st_uid);
                if ((st.st_uid % 100000) >= 90000) {
                    PTRACE_LOG("is isolated process\n");
                    if (sulist_enabled)
                        goto DETACH_PROC;
                    goto CHECK_PROC;
                }

                // check if UID is on list
                if (!is_uid_on_list(st.st_uid))
                    goto DETACH_PROC;

                CHECK_PROC:
                    checked[pid] = true;
                if (!allowed[pid] && (
                        // app zygote
                        strstr(get_content(pid, "attr/current").data(), "u:r:app_zygote:s0") ||
                        // until pre-initialized
                        get_content(pid, "cmdline") == "<pre-initialized>"))
                    allowed[pid] = true;

                if (!allowed[pid])
                    continue;

                if (check_pid(pid))
                    goto skip;
                continue;

                DETACH_PROC:
                    detach_pid(pid);
                goto skip;
            } while (false);
            xptrace(PTRACE_SYSCALL, pid);
        } else if (signal == SIGSTOP) {
            // SIGSTOP is produced by ptrace
            if (!attaches[pid]) {
                // Double check if this is actually a process
                attaches[pid] = is_process(pid);
            }
            if (attaches[pid]) {
                // This is a process, continue monitoring
                PTRACE_LOG("SIGSTOP from child\n");
                xptrace(PTRACE_SETOPTIONS, pid, nullptr,
                    PTRACE_O_TRACESYSGOOD);
                // 在目标进程栈上注入 BPF allow-all 过滤器绕过 seccomp，
                // 确保 denylist ptrace 追踪不被 seccomp 拦截。
                remote_bypass_seccomp(pid);
                xptrace(PTRACE_SYSCALL, pid);
            } else {
                // This is a thread, do NOT monitor
                PTRACE_LOG("SIGSTOP from thread\n");
                DETACH_AND_CONT;
            }
        } else {
            // Not caused by us, resend signal
            xptrace((!zygote_map.count(pid) && attaches[pid]) ? 
                    PTRACE_SYSCALL : PTRACE_CONT, pid, nullptr, signal);
            PTRACE_LOG("signal [%d]\n", signal);
        }

        skip:
            continue;
    }
}
