// Adapted from Kokoro-no-kitsune-27001b: native/src/core/zygisk/proc_monitor.cpp
// Original author: 5ec1cff (ZygiskNext)

#include <base.hpp>
#include <unistd.h>
#include <signal.h>
#include <sys/ptrace.h>
#include <sys/wait.h>
#include <string>
#include <set>
#include <atomic>

#include <core.hpp>
#include <consts.hpp>

#include "zygisk.hpp"

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

static void inject_zygote(int pid) {
    auto program = get_program(pid);
    if (program != "/system/bin/app_process" &&
        program != "/system/bin/app_process32" &&
        program != "/system/bin/app_process64") return;

    LOGI("zygisk: inject zygote PID=[%d] [%s]\n", pid, program.c_str());

    auto tracer = string(get_magisk_tmp()) + "/magisk";
    auto pid_str = to_string(pid);

    kill(pid, SIGSTOP);
    ptrace(PTRACE_CONT, pid, 0, 0);
    int status;
    waitpid(pid, &status, __WALL);

    if (WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP && (status >> 16) == 0) {
        ptrace(PTRACE_DETACH, pid, 0, SIGSTOP);
        // 调试：检查 tracer 路径是否可执行
        LOGI("zygisk: tracer path=[%s]\n", tracer.c_str());
        if (access(tracer.c_str(), X_OK) == -1) {
            LOGE("zygisk: tracer %s not accessible: %s\n", tracer.c_str(), strerror(errno));
        }
        auto p = xfork();
        if (p == 0) {
            execl(tracer.c_str(), "", "zygisk", "trace_zygote",
                  pid_str.c_str(), tracer.c_str(), nullptr);
            LOGE("zygisk: exec %s failed: %s\n", tracer.c_str(), strerror(errno));
            kill(pid, SIGKILL);
            _exit(1);
        } else if (p > 0) {
            int child_status;
            for (int i = 0; i < 50; i++) {
                int ret = waitpid(p, &child_status, WNOHANG);
                if (ret == p) {
                    if (WIFEXITED(child_status)) {
                        int code = WEXITSTATUS(child_status);
                        if (code == 0) {
                            LOGI("zygisk: trace_zygote done for PID %d\n", pid);
                        } else {
                            LOGW("zygisk: trace_zygote PID %d fail step=%d\n", pid, code);
                        }
                    } else if (WIFSIGNALED(child_status)) {
                        LOGW("zygisk: trace_zygote PID %d killed (sig=%d)\n", pid, WTERMSIG(child_status));
                    }
                    goto inject_done;
                }
                usleep(100000);
            }
            LOGW("zygisk: trace_zygote timeout for PID %d\n", pid);
        inject_done:;
        } else {
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
            LOGI("zygisk: polling found zygote PID=[%d]\n", pid);
            auto tracer = string(get_magisk_tmp()) + "/magisk";
            auto pid_str = to_string(pid);
            auto p = xfork();
            if (p == 0) {
                execl(tracer.c_str(), "", "zygisk", "trace_zygote",
                      pid_str.c_str(), tracer.c_str(), nullptr);
                PLOGE("failed to exec");
                _exit(1);
            } else if (p > 0) {
                int child_status;
                for (int i = 0; i < 50; i++) {
                    int ret = waitpid(p, &child_status, WNOHANG);
                    if (ret == p) {
                        if (WIFEXITED(child_status)) {
                            int code = WEXITSTATUS(child_status);
                            if (code != 0) {
                                LOGW("zygisk: poll tracer PID %d fail step=%d\n", pid, code);
                            }
                        }
                        goto poll_done;
                    }
                    usleep(100000);
                }
                LOGW("zygisk: poll tracer timeout for PID %d\n", pid);
                poll_done:;
            } else {
                PLOGE("failed to fork");
                continue;
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
