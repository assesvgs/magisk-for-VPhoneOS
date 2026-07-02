#pragma once

#include <sys/socket.h>
#include <string>
#include <vector>
#include <atomic>
#include <functional>
#include <poll.h>

#include <base.hpp>

#include "../core-rs.hpp"

#define AID_ROOT   0
#define AID_SHELL  2000
#define AID_USER_OFFSET 100000

#define to_app_id(uid)  (uid % AID_USER_OFFSET)
#define to_user_id(uid) (uid / AID_USER_OFFSET)

#define SDK_INT      (MagiskD::Get().sdk_int())
#define APP_DATA_DIR (SDK_INT >= 24 ? "/data/user_de" : "/data/user")

inline int connect_daemon(RequestCode req) {
    return connect_daemon(req, false);
}

// Multi-call entrypoints
int su_client_main(int argc, char *argv[]);

struct ModuleInfo;

struct module_info {
    std::string name;
    int z32 = -1;
#if defined(__LP64__)
    int z64 = -1;
#endif
};
extern std::vector<module_info> *module_list;

// Utils
const char *get_magisk_tmp();
void unlock_blocks();
bool check_key_combo();
template<typename T> requires(std::is_trivially_copyable_v<T>)
T read_any(int fd) {
    T val;
    if (xxread(fd, &val, sizeof(val)) != sizeof(val))
        return -1;
    return val;
}
template<typename T> requires(std::is_trivially_copyable_v<T>)
void write_any(int fd, T val) {
    if (fd < 0) return;
    xwrite(fd, &val, sizeof(val));
}
inline int read_int(int fd) { return read_any<int>(fd); }
inline void write_int(int fd, int val) { write_any(fd, val); }
std::string read_string(int fd);
bool read_string(int fd, std::string &str);
void write_string(int fd, std::string_view str);
template<typename T> requires(std::is_trivially_copyable_v<T>)
void write_vector(int fd, const std::vector<T> &vec) {
    write_int(fd, vec.size());
    xwrite(fd, vec.data(), vec.size() * sizeof(T));
}
template<typename T> requires(std::is_trivially_copyable_v<T>)
bool read_vector(int fd, std::vector<T> &vec) {
    int size = read_int(fd);
    vec.resize(size);
    return xread(fd, vec.data(), size * sizeof(T)) == size * sizeof(T);
}

// Scripting
void install_apk(Utf8CStr apk);
void uninstall_pkg(Utf8CStr pkg);
void exec_common_scripts(Utf8CStr stage);
void exec_module_scripts(Utf8CStr stage, const rust::Vec<ModuleInfo> &module_list);
void exec_script(Utf8CStr script);
void clear_pkg(const char *pkg, int user_id);
[[noreturn]] void install_module(Utf8CStr file);

// Denylist
extern std::atomic<bool> denylist_enforced;
extern bool sulist_enabled;
int denylist_cli(rust::Vec<rust::String> &args);
void denylist_handler(int client);
void initialize_denylist();
void scan_deny_apps();
bool is_deny_target(int uid, std::string_view process, int max_len = 0);

void revert_unmount(int pid = -1) noexcept;

// Kitsune Mask specific
extern int magisktmpfs_fd;
extern bool HAVE_32;
extern int su_bin_fd;
extern bool logging_muted;
void su_mount();
void mount_mirrors();
void enable_mount_su();
int tmpfs_mount(const char *from, const char *to);
int bind_mount_(const char *from, const char *to);
void do_mount_magisk(int pid);
int get_su_bin_fd();
int get_magisktmpfs_fd();

// Zygisk monitoring FFI (called from Rust via cxx bridge)
void start_zygisk_monitor();
void set_zygisk_stop_tracing(bool stop);
bool trace_zygote(int pid, rust::Str libpath);

// mount_info 和 parse_mount_info 已迁移至 Rust (base/files.rs)

// MagiskSU
void exec_root_shell(int client, int pid, SuRequest &req, MntNsMode mode);

void check_pkg_refresh();
int get_manager(int user_id = 0, std::string *pkg = nullptr, bool install = false);

struct sock_cred : public ucred {
    std::string context;
};
bool get_client_cred(int fd, sock_cred *cred);
int send_fds(int sockfd, const int *fds, int cnt);

// Rust bindings
inline Utf8CStr get_magisk_tmp_rs() { return get_magisk_tmp(); }
inline rust::String resolve_preinit_dir_rs(Utf8CStr base_dir) {
    return resolve_preinit_dir(base_dir.c_str());
}

void update_deny_flags(int uid, rust::Str process, uint32_t &flags);
void zygiskd_companion_entry(int socket);

// Rust thread pool bridge
extern "C" void exec_task_from_cxx(void (*func)(void*), void *arg);

template<typename F>
void exec_task(F&& fn) noexcept {
    using decayed = std::decay_t<F>;
    auto *ctx = new decayed(std::forward<F>(fn));
    exec_task_from_cxx([](void *arg) noexcept {
        auto *ctx = static_cast<decayed*>(arg);
        (*ctx)();
        delete ctx;
    }, ctx);
}
