#pragma once

#include <string_view>
#include <atomic>

#include <core.hpp>

#define ISOLATED_MAGIC "isolated"

namespace DenyRequest {
enum : int {
    ENFORCE,
    DISABLE,
    ADD,
    REMOVE,
    LIST,
    STATUS,
    SULIST_STATUS,
    ENFORCE_SULIST,
    DISABLE_SULIST,

    END
};
}

namespace DenyResponse {
enum : int {
    OK,
    ENFORCED,
    NOT_ENFORCED,
    ITEM_EXIST,
    ITEM_NOT_EXIST,
    INVALID_PKG,
    NO_NS,
    ERROR,
    SULIST_ENFORCED,
    SULIST_NOT_ENFORCED,
    SULIST_NO_DISABLE,

    END
};
}

// CLI entries
int enable_deny();
int disable_deny();
int add_list(int client);
int rm_list(int client);
void ls_list(int client);

bool proc_context_match(int pid, std::string_view context);
void *logcat(void *arg);
extern bool logcat_exit;

// Sulist functions
bool is_uid_on_sulist(int uid);
int mount_magisk_for_pid(int pid);
int unmount_magisk_for_pid(int pid);
bool is_sulist_enabled();
void update_sulist_config(bool enable);
void initialize_denylist();
bool is_deny_target(int uid, std::string_view process, int max_len = 0);
bool is_uid_on_list(int uid);
void rescan_apps();

// Global variables
extern bool sulist_enabled;
extern atomic<bool> denylist_enforced;

// Deny flags update function
void update_deny_flags(int uid, rust::Str process, uint32_t &flags);
