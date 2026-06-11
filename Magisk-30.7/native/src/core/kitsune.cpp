// Kitsune Mask specific variables and functions
// This file implements the missing functionality from KitsuneMag-kitsune

#include <sys/mount.h>
#include <sys/stat.h>
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

// tmpfs_mount function from KitsuneMag-kitsune
static int tmpfs_mount(const char *from, const char *to) {
    return xmount(from, to, "tmpfs", 0, "mode=755");
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
