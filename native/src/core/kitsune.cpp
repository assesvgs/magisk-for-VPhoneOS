// Kitsune Mask specific variables and functions
// This file implements the missing functionality from KitsuneMag-kitsune

#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>
#include <fcntl.h>
#include <string>

#include <core.hpp>
#include <base.hpp>
#include <consts.hpp>

using namespace std;

// Global variables from KitsuneMag-kitsune
int magisktmpfs_fd = -1;
bool HAVE_32 = false;
int su_bin_fd = -1;
bool logging_muted = false;

// tmpfs_mount function from KitsuneMag-kitsune
int tmpfs_mount(const char *from, const char *to) {
    return xmount(from, to, "tmpfs", 0, "mode=755");
}

// bind_mount_ function from KitsuneMag-kitsune
int bind_mount_(const char *from, const char *to) {
    return xmount(from, to, nullptr, MS_BIND, nullptr);
}

// parse_mount_info from KitsuneMag-kitsune
std::vector<mount_info> parse_mount_info(const char *pid) {
    char buf[PATH_MAX] = {};
    ssprintf(buf, sizeof(buf), "/proc/%s/mountinfo", pid);
    std::vector<mount_info> result;

    auto fp = open_file(buf, "re");
    if (!fp) return result;

    char line[4096];
    while (fgets(line, sizeof(line), fp.get())) {
        mount_info info{};
        int root_start = 0, root_end = 0;
        int target_start = 0, target_end = 0;
        int type_start = 0, type_end = 0;
        int source_start = 0, source_end = 0;
        int fs_option_start = 0, fs_option_end = 0;
        unsigned int id, parent, maj, min;
        sscanf(line,
               "%u %u %u:%u %n%*s%n %n%*s%n %n%*s%n - %n%*s%n %n%*s%n %n%*s%n",
               &id, &parent, &maj, &min,
               &root_start, &root_end,
               &target_start, &target_end,
               &type_start, &type_end,
               &source_start, &source_end,
               &fs_option_start, &fs_option_end);
        info.id = id;
        info.parent = parent;
        info.device = makedev(maj, min);
        info.root = std::string(line + root_start, root_end - root_start);
        info.target = std::string(line + target_start, target_end - target_start);
        info.type = std::string(line + type_start, type_end - type_start);
        info.source = std::string(line + source_start, source_end - source_start);
        info.fs_option = std::string(line + fs_option_start, fs_option_end - fs_option_start);
        result.push_back(std::move(info));
    }
    return result;
}

// mount_su function from KitsuneMag-kitsune
// This function mounts MagiskSU binaries into the system
static int mount_su() {
    char buf[4096];
    ssprintf(buf, sizeof(buf), "%s/" WORKERDIR, get_magisk_tmp());
    if (xmount("magisk", buf, "tmpfs", 0, "mode=755"))
        return -1;
    xmount(nullptr, buf, nullptr, MS_PRIVATE, nullptr);

    // Create a simple fd that points to /system/bin
    struct stat st_src{}, st_dest{};
    stat(buf, &st_src);
    stat("/system/bin", &st_dest);
    umount2(buf, MNT_DETACH);

    int fd = (st_src.st_dev == st_dest.st_dev) ?
        xopen("/system/bin", O_PATH | O_CLOEXEC) : -1;

    ssprintf(buf, sizeof(buf), "/proc/self/fd/%d", fd);
    xmount(nullptr, buf, nullptr, MS_REMOUNT | MS_RDONLY, nullptr);

    return fd;
}

// enable_mount_su function from KitsuneMag-kitsune
// This function is called during boot-complete to mount MagiskSU
void enable_mount_su() {
    if (su_bin_fd < 0) {
        LOGI("* Mount MagiskSU\n");
        su_bin_fd = mount_su();

        char buf[128];
        ssprintf(buf, sizeof(buf), "/proc/self/fd/%d", su_bin_fd);
        xmount(nullptr, buf, nullptr, MS_SHARED, nullptr);
    }
}

// disable_unmount_su function from KitsuneMag-kitsune
void disable_unmount_su() {
    if (su_bin_fd >= 0) {
        LOGI("* Unmount MagiskSU\n");
        char buf[128];
        ssprintf(buf, sizeof(buf), "/proc/self/fd/%d", su_bin_fd);
        umount2(buf, MNT_DETACH);
        close(su_bin_fd);
        su_bin_fd = -1;
    }
}

// su_mount function from KitsuneMag-kitsune
// This function is called from do_mount_magisk to mount MagiskSU for a specific process
void su_mount() {
    // In the original Kitsune Mag, this calls load_modules(true) and mount_su()
    // For now, we just call enable_mount_su()
    enable_mount_su();
}

// mount_mirrors function placeholder
// This function is declared but not used in revert.cpp
void mount_mirrors() {
    LOGD("mount_mirrors: placeholder\n");
}
