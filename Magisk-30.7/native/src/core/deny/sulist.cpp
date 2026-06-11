// Sulist functionality for MagiskHide
// This file implements the sulist feature from KitsuneMag-kitsune

#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>
#include <fcntl.h>
#include <dirent.h>
#include <set>
#include <map>
#include <string>
#include <vector>
#include <atomic>
#include <mutex>

#include <core.hpp>
#include <base.hpp>

#include "deny.hpp"

using namespace std;

// Global variables
bool sulist_enabled = false;
atomic<bool> denylist_enforced = false;
static const char *table_name = "hidelist";

// Data structures for package/process tracking
static map<string, set<string, StringCmp>, StringCmp> pkg_to_procs;
static map<int, set<string_view>> app_id_to_pkgs;
static pthread_mutex_t data_lock = PTHREAD_MUTEX_INITIALIZER;

// Helper function to check if a string starts with a prefix
static bool str_starts(string_view str, string_view prefix) {
    return str.substr(0, prefix.length()) == prefix;
}

// Rescan installed apps
void rescan_apps() {
    LOGD("denylist: rescanning apps\n");

    if (sulist_enabled) {
        // Add manager package to sulist
        add_hide_set(JAVA_PACKAGE_NAME, JAVA_PACKAGE_NAME);
    }

    app_id_to_pkgs.clear();

    auto data_dir = xopen_dir(APP_DATA_DIR);
    if (!data_dir)
        return;

    dirent *entry;
    while ((entry = xreaddir(data_dir.get()))) {
        // For each user
        int dfd = xopenat(dirfd(data_dir.get()), entry->d_name, O_RDONLY);
        if (auto dir = xopen_dir(dfd)) {
            while ((entry = xreaddir(dir.get()))) {
                struct stat st{};
                // For each package
                if (fstatat(dfd, entry->d_name, &st, 0))
                    continue;
                int app_id = to_app_id(st.st_uid);
                if (auto it = pkg_to_procs.find(entry->d_name); it != pkg_to_procs.end()) {
                    app_id_to_pkgs[app_id].insert(it->first);
                }
            }
        } else {
            close(dfd);
        }
    }
}

// Add a package/process to the hide list
static bool add_hide_set(const char *pkg, const char *proc) {
    auto p = pkg_to_procs[pkg].emplace(proc);
    if (!p.second)
        return false;
    LOGI("denylist add: [%s/%s]\n", pkg, proc);
    return true;
}

// Remove a package/process from the hide list
static bool remove_hide_set(const char *pkg, const char *proc) {
    auto it = pkg_to_procs.find(pkg);
    if (it == pkg_to_procs.end())
        return false;
    auto p = it->second.erase(proc);
    if (p == 0)
        return false;
    if (it->second.empty()) {
        pkg_to_procs.erase(it);
    }
    LOGI("denylist rm: [%s/%s]\n", pkg, proc);
    return true;
}

// Check if a process is a deny target
bool is_deny_target(int uid, string_view process, int max_len) {
    mutex_guard lock(data_lock);

    int app_id = to_app_id(uid);

    if (app_id >= 90000) {
        // Isolated process
        if (auto it = pkg_to_procs.find(ISOLATED_MAGIC); it != pkg_to_procs.end()) {
            for (const auto &s : it->second) {
                if (s.length() > max_len && process.length() > max_len && str_starts(s, process))
                    return true;
                if (str_starts(process, s))
                    return true;
            }
        }
        return false;
    } else {
        auto it = app_id_to_pkgs.find(app_id);
        if (it == app_id_to_pkgs.end())
            return false;
        for (const auto &pkg : it->second) {
            if (pkg_to_procs.find(pkg)->second.count(process))
                return true;
        }
        for (const auto &s : it->second) {
            if (s.length() > max_len && process.length() > max_len && str_starts(s, process))
                return true;
            if (s == process)
                return true;
        }
    }
    return false;
}

// Check if a UID is on the list
bool is_uid_on_list(int uid) {
    auto it = app_id_to_pkgs.find(uid % 100000);
    if (it == app_id_to_pkgs.end())
        return false;
    for (const auto &pkg : it->second) {
        if (pkg_to_procs.find(pkg)->second.size() > 0)
            return true;
    }
    return false;
}

// Enable denylist
int enable_deny() {
    if (denylist_enforced) {
        return DenyResponse::OK;
    }

    mutex_guard lock(data_lock);

    if (access("/proc/self/ns/mnt", F_OK) != 0) {
        LOGW("The kernel does not support mount namespace\n");
        sulist_enabled = false;
        table_name = "hidelist";
        update_sulist_config(false);
        return DenyResponse::NO_NS;
    }

    if (sulist_enabled) {
        LOGI("* Enable SuList\n");
    } else {
        LOGI("* Enable MagiskHide\n");
    }

    denylist_enforced = true;

    if (sulist_enabled) {
        // Add SystemUI and Settings to sulist because modules might need to modify it
        add_hide_set("com.android.systemui", "com.android.systemui");
        add_hide_set("com.android.settings", "com.android.settings");
        add_hide_set(JAVA_PACKAGE_NAME, JAVA_PACKAGE_NAME);
    }

    return DenyResponse::OK;
}

// Disable denylist
int disable_deny() {
    // sulist mode cannot be turn off without reboot
    if (sulist_enabled)
        return DenyResponse::SULIST_NO_DISABLE;

    if (denylist_enforced) {
        denylist_enforced = false;
        LOGI("* Disable MagiskHide\n");
    }

    return DenyResponse::OK;
}

// Add a package/process to the list
int add_list(int client) {
    string pkg = read_string(client);
    string proc = read_string(client);

    if (pkg.empty() || proc.empty()) {
        return DenyResponse::INVALID_PKG;
    }

    mutex_guard lock(data_lock);

    if (add_hide_set(pkg.data(), proc.data())) {
        return DenyResponse::OK;
    } else {
        return DenyResponse::ITEM_EXIST;
    }
}

// Remove a package/process from the list
int rm_list(int client) {
    string pkg = read_string(client);
    string proc = read_string(client);

    if (pkg.empty()) {
        return DenyResponse::INVALID_PKG;
    }

    mutex_guard lock(data_lock);

    if (proc.empty()) {
        // Remove all processes for this package
        auto it = pkg_to_procs.find(pkg);
        if (it == pkg_to_procs.end()) {
            return DenyResponse::ITEM_NOT_EXIST;
        }
        pkg_to_procs.erase(it);
        LOGI("denylist rm: [%s/*]\n", pkg.data());
        return DenyResponse::OK;
    } else {
        if (remove_hide_set(pkg.data(), proc.data())) {
            return DenyResponse::OK;
        } else {
            return DenyResponse::ITEM_NOT_EXIST;
        }
    }
}

// List all packages/processes
void ls_list(int client) {
    mutex_guard lock(data_lock);

    for (const auto &[pkg, procs] : pkg_to_procs) {
        for (const auto &proc : procs) {
            string entry = pkg + "/" + string(proc);
            write_string(client, entry);
        }
    }
    write_string(client, ""); // End of list
}

// Update sulist configuration
void update_sulist_config(bool enable) {
    // This is a placeholder - the actual implementation needs
    // to interact with the database
    sulist_enabled = enable;
}

// Initialize denylist
void initialize_denylist() {
    // This is a placeholder - the actual implementation needs
    // to read from the database
}

// Scan deny apps (called from daemon)
void scan_deny_apps() {
    mutex_guard lock(data_lock);
    rescan_apps();
}

// Mount Magisk for a specific process
int mount_magisk_for_pid(int pid) {
    // This is a placeholder - the actual implementation needs
    // to be ported from KitsuneMag-kitsune's revert.cpp
    return 0;
}

// Unmount Magisk for a specific process
int unmount_magisk_for_pid(int pid) {
    // Call the Rust revert_unmount function
    revert_unmount(pid);
    return 0;
}

// Check if sulist is enabled
bool is_sulist_enabled() {
    return sulist_enabled;
}

// Check if a UID is on the sulist
bool is_uid_on_sulist(int uid) {
    return is_uid_on_list(uid);
}

// Update deny flags for a process
void update_deny_flags(int uid, rust::Str process, uint32_t &flags) {
    string_view proc_view(process.begin(), process.end());

    if (sulist_enabled) {
        // In sulist mode, processes ON the list should be allowed
        if (is_deny_target(uid, proc_view)) {
            flags |= +ZygiskStateFlags::ProcessOnAllowList;
        }
        flags |= +ZygiskStateFlags::AllowlistEnforcing;
        // If not on allowlist, treat as denylist
        if (!(flags & +ZygiskStateFlags::ProcessOnAllowList)) {
            flags |= +ZygiskStateFlags::ProcessOnDenyList;
        }
    } else {
        // In normal denylist mode
        if (is_deny_target(uid, proc_view)) {
            flags |= +ZygiskStateFlags::ProcessOnDenyList;
        }
    }

    if (denylist_enforced) {
        flags |= +ZygiskStateFlags::DenyListEnforced;
    }
}
