// Adapted from Kokoro-no-kitsune-27001b: native/src/core/zygisk/proc_monitor.cpp
// Original author: 5ec1cff (ZygiskNext)

#include <base.hpp>
#include <unistd.h>
#include <signal.h>
#include <sys/ptrace.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <string>
#include <set>
#include <atomic>

#include <core.hpp>
#include <consts.hpp>

#include "zygisk.hpp"
#include "ptrace_utils.hpp"

using namespace std;

#define WEVENT(__status) (((__status) >> 16) & 0xff)

static atomic<bool> stop_tracing{false};

void set_zygisk_stop_tracing(bool stop) {
    stop_tracing.store(stop);
}

static string get_program(int pid) {
    char path[32];
    char buf[256];
    ssprintf(path, sizeof(path), "/proc/%d/exe", pid);
    auto sz = readlink(path, buf, sizeof(buf) - 1);
    if (sz < 0) return "";
    buf[sz] = '\0';
    return buf;
}

// Ensure magisk is available in MAGISKTMP for tracer
// (bootstages.rs copies magisk32/zygisk_inject but not magisk itself)
static void ensure_magisk_tracer() {
    auto tracer_path = string(get_magisk_tmp()) + "/magisk";
    if (access(tracer_path.c_str(), X_OK) == 0) return;
    auto magisk_src = string(DATABIN) + "/magisk";
    if (access(magisk_src.c_str(), F_OK) != 0) return;
    int src_fd = xopen(magisk_src.c_str(), O_RDONLY | O_CLOEXEC);
    if (src_fd < 0) return;
    auto content = full_read(src_fd);
    close(src_fd);
    int dst_fd = xopen(tracer_path.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0755);
    if (dst_fd >= 0) {
        auto written = write(dst_fd, content.data(), content.size());
        close(dst_fd);
        if (written == static_cast<ssize_t>(content.size())) {
            LOGD("zygisk: copied magisk tracer to %s (%zu bytes)\n",
                 tracer_path.c_str(), content.size());
        } else {
            LOGW("zygisk: partial write magisk tracer %zd/%zu bytes, unlink\n",
                 written, content.size());
            unlink(tracer_path.c_str());
        }
    }
}

// Fork, exec tracer, and wait for result.
// Returns: 0=success, >0=fail step, -2=signaled, -3=unexpected, -4=timeout.
// On fork failure returns -1 (caller must log and handle).
// If kill_on_fail > 0, kills that PID when exec fails in child.
static int exec_tracer(const string &tracer, const string &pid_str,
                       const string &inject_lib, const char *log_prefix,
                       int kill_on_fail = 0) {
    auto p = xfork();
    if (p == 0) {
        TRACELOGW("%s child exec %s\n", log_prefix, tracer.c_str());
        execl(tracer.c_str(), "magisk", "--zygisk", "trace_zygote",
              pid_str.c_str(), inject_lib.c_str(), nullptr);
        PLOGE("zygisk: %s exec %s", log_prefix, tracer.c_str());
        if (kill_on_fail > 0) kill(kill_on_fail, SIGKILL);
        _exit(1);
    } else if (p > 0) {
        int child_status;
        for (int i = 0; i < 50; i++) {
            int ret = waitpid(p, &child_status, WNOHANG);
            if (ret != p) {
                usleep(100000);
                continue;
            }
            if (WIFEXITED(child_status)) {
                int code = WEXITSTATUS(child_status);
                TRACELOGW("%s tracer exit code=%d for pid=%s\n", log_prefix, code, pid_str.c_str());
                return code;
            } else if (WIFSIGNALED(child_status)) {
                TRACELOGW("%s tracer killed sig=%d for pid=%s\n", log_prefix,
                          WTERMSIG(child_status), pid_str.c_str());
                LOGW("zygisk: %s tracer PID %s killed (sig=%d)\n",
                     log_prefix, pid_str.c_str(), WTERMSIG(child_status));
                return -2;
            }
            return -3;
        }
        LOGW("zygisk: %s tracer timeout for PID %s\n", log_prefix, pid_str.c_str());
        return -4;
    }
    return -1;
}

static void inject_zygote(int pid) {
    auto program = get_program(pid);
    if (program != "/system/bin/app_process" &&
        program != "/system/bin/app_process32" &&
        program != "/system/bin/app_process64") return;

    LOGI("zygisk: inject zygote PID=[%d] [%s]\n", pid, program.c_str());
    TRACELOGW("inject: enter pid=%d program=%s\n", pid, program.c_str());

    auto tracer = string(get_magisk_tmp()) + "/magisk";
    if (program == "/system/bin/app_process32")
        tracer = string(get_magisk_tmp()) + "/magisk32";
    TRACELOGW("inject: tracer=%s\n", tracer.c_str());
    auto pid_str = to_string(pid);
    string inject_lib;
    if (program.find("32") != string::npos) {
        inject_lib = string(get_magisk_tmp()) + "/zygisk_inject32";
    } else {
        inject_lib = string(get_magisk_tmp()) + "/zygisk_inject";
    }

    ensure_magisk_tracer();
    if (access(tracer.c_str(), X_OK) == -1) {
        LOGW("zygisk: skip injection PID=%d tracer=%s (%s)\n",
             pid, tracer.c_str(), strerror(errno));
        return;
    }

    kill(pid, SIGSTOP);
    int status;
    if (ptrace(PTRACE_CONT, pid, 0, 0) == -1) {
        PLOGE("ptrace CONT in inject_zygote");
        ptrace(PTRACE_DETACH, pid, 0, 0);
        return;
    }
    waitpid(pid, &status, __WALL);

    if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP && (status >> 16) == 0) {
        ptrace(PTRACE_DETACH, pid, 0, SIGSTOP);
        LOGI("zygisk: tracer path=[%s]\n", tracer.c_str());
        TRACELOGW("inject: fork+exec for pid=%d\n", pid);
        int ret = exec_tracer(tracer, pid_str, inject_lib, "inject", pid);
        if (ret == 0) {
            LOGI("zygisk: trace_zygote done for PID %d\n", pid);
        } else if (ret > 0) {
            LOGW("zygisk: trace_zygote PID %d fail step=%d\n", pid, ret);
        } else if (ret == -1) {
            PLOGE("failed to fork");
            kill(pid, SIGKILL);
        }
    } else {
        ptrace(PTRACE_DETACH, pid, 0, 0);
    }
}

static bool find_zygote_by_polling() {
    // VPhoneOS fallback when PTRACE_SEIZE init(1) fails
    // Nobody is tracing this process (no TRACEFORK inheritance).
    // Use readdir(/proc) to scan all PIDs, analogous to crawl_procfs.
    auto dir = xopen_dir("/proc");
    if (!dir) return false;
    dirent *dp;
    while ((dp = xreaddir(dir.get()))) {
        int pid = parse_int(dp->d_name);
        if (pid <= 0) continue;
        auto program = get_program(pid);
        if (program == "/system/bin/app_process" ||
            program == "/system/bin/app_process32" ||
            program == "/system/bin/app_process64") {
            if (stop_tracing.load()) {
                LOGI("zygisk: stop_tracing set, skip polling injection PID %d\n", pid);
                return false;
            }
            LOGI("zygisk: polling found zygote PID=[%d]\n", pid);
            auto tracer = string(get_magisk_tmp()) + "/magisk";
            if (program == "/system/bin/app_process32")
                tracer = string(get_magisk_tmp()) + "/magisk32";
            auto pid_str = to_string(pid);
            ensure_magisk_tracer();
            if (access(tracer.c_str(), X_OK) == -1) {
                LOGW("zygisk: poll skip injection PID=%d tracer=%s (%s)\n",
                     pid, tracer.c_str(), strerror(errno));
                continue;
            }
            // 注意：此处不执行 SIGSTOP 同步（与 inject_zygote 不同），原因：
            // polling 路径不是 zygote 的 tracer（未从 init 继承 PTRACE_O_TRACEFORK），
            // 无法对其执行 PTRACE_CONT/waitpid。但子 tracer 使用 PTRACE_SEIZE 附加，
            // 不需要目标先进入停止状态。SIGSTOP 在此路径会阻塞 zygote 且无法恢复。
            string inject_lib;
            if (program.find("32") != string::npos) {
                inject_lib = string(get_magisk_tmp()) + "/zygisk_inject32";
            } else {
                inject_lib = string(get_magisk_tmp()) + "/zygisk_inject";
            }
            int ret = exec_tracer(tracer, pid_str, inject_lib, "inject: poll");
            if (ret == -1) {
                PLOGE("failed to fork");
                continue;
            } else if (ret > 0) {
                LOGW("zygisk: poll tracer PID %d fail step=%d\n", pid, ret);
            }
            return true;
        }
    }
    return false;
}

extern "C" void *init_monitor(void *) {
    LOGI("zygisk: init_monitor starting\n");

    int status;
    set<pid_t> process;

    if (ptrace(PTRACE_SEIZE, 1, 0, PTRACE_O_TRACEFORK) == -1) {
        LOGW("zygisk: PTRACE_SEIZE init(1) failed, falling back to polling\n");
        // VPhoneOS fallback: keep running, check stop_tracing before injection
        while (true) {
            if (!stop_tracing.load()) {
                if (find_zygote_by_polling())
                    break;
            }
            sleep(1);
        }
        LOGI("zygisk: init_monitor polling path exited\n");
        return nullptr;
    }

    LOGI("zygisk: start tracing init\n");

    while (true) {
        int pid;
        while ((pid = waitpid(-1, &status, __WALL | __WNOTHREAD)) > 0) {
            if (pid == 1) {
                if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP &&
                    WEVENT(status) == PTRACE_EVENT_FORK) {
                    long child_pid;
                    if (ptrace(PTRACE_GETEVENTMSG, pid, 0, &child_pid) == -1) {
                        PLOGE("PTRACE_GETEVENTMSG");
                        child_pid = -1;
                    }
                    LOGD("zygisk: init forked %ld\n", child_pid);
                }
                if (WIFSTOPPED(status)) {
                    ptrace(PTRACE_CONT, pid, 0,
                           (WEVENT(status) == 0) ? WSTOPSIG(status) : 0);
                }
                continue;
            }

            auto state = process.find(pid);
            if (state == process.end()) {
                LOGD("zygisk: attached pid=%d\n", pid);
                process.emplace(pid);
                if (ptrace(PTRACE_SETOPTIONS, pid, 0, PTRACE_O_TRACEEXEC) == -1) {
                    PLOGE("PTRACE_SETOPTIONS");
                    process.erase(pid);
                    ptrace(PTRACE_DETACH, pid, 0, 0);
                    // waitpid 刚返回 → ptrace 关系已确认存在，双故障概率极低
                    // 若 DETACH 也失败，进程进入无主 TRACED 状态。
                    // 概率极低且不可恢复，不加 kill 兜底以免增加代码噪声。
                    continue;
                }
                ptrace(PTRACE_CONT, pid, 0, 0);
                continue;
            }

            if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP &&
                WEVENT(status) == PTRACE_EVENT_EXEC) {
                auto program = get_program(pid);
                LOGD("zygisk: pid=[%d] [%s]\n", pid, program.c_str());

                if (program == "/system/bin/app_process" ||
                    program == "/system/bin/app_process32" ||
                    program == "/system/bin/app_process64") {
                    if (!stop_tracing.load()) {
                        inject_zygote(pid);
                    }
                }
                process.erase(state);
                if (WIFSTOPPED(status)) {
                    ptrace(PTRACE_DETACH, pid, 0, 0);
                }
            } else {
                process.erase(state);
                if (WIFSTOPPED(status)) {
                    ptrace(PTRACE_DETACH, pid, 0, 0);
                }
            }
        }
        if (errno == ECHILD) {
            struct timespec ts = { .tv_sec = INT_MAX, .tv_nsec = 0 };
            nanosleep(&ts, nullptr);
        }
    }

    LOGI("zygisk: init_monitor exited\n");
    return nullptr;
}

void start_zygisk_monitor() {
    LOGI("zygisk: starting init_monitor\n");
    new_daemon_thread(reinterpret_cast<thread_entry>(&init_monitor));
}
