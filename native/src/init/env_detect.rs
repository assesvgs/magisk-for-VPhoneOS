//! 系统环境探测模块
//! 
//! 用于在 magiskinit 第一阶段收集系统环境信息
//! 所有日志使用 debug!() 宏，仅在 debug 构建生效

use base::{debug, cstr, libc, parse_mount_info, nix};
use std::fs;

/// 探测系统环境（在 first_stage 开始时调用）
pub fn detect_system_environment() {
    debug!("=== System Environment Detection Start ===");
    
    detect_kernel_info();
    detect_device_info();
    detect_android_version();
    detect_virtual_machine();
    detect_namespace_info();
    detect_filesystem_info();
    detect_mount_info();
    detect_block_devices();
    detect_critical_directories();
    detect_encryption_status();
    detect_selinux_status();
    detect_partition_info();
    detect_boot_mode();
    detect_magisk_environment();
    
    debug!("=== System Environment Detection End ===");
}

/// 内核信息探测
fn detect_kernel_info() {
    debug!("--- Kernel Info ---");
    
    // 内核版本
    if let Ok(version) = fs::read_to_string("/proc/version") {
        debug!("kernel.version={}", version.trim());
    }
    
    // cmdline
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        debug!("kernel.cmdline={}", cmdline.trim());
    }
    
    // osrelease
    if let Ok(osrelease) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        debug!("kernel.osrelease={}", osrelease.trim());
    }
}

/// 设备信息探测
fn detect_device_info() {
    debug!("--- Device Info ---");
    
    let props = [
        "ro.product.model",
        "ro.hardware", 
        "ro.product.cpu.abi",
        "ro.product.device",
        "ro.product.brand",
        "ro.product.manufacturer",
        "ro.build.display.id",
        "ro.build.fingerprint",
    ];
    
    for prop in props {
        let value = get_prop(prop);
        debug!("device.{}={}", prop, value);
    }
}

/// Android 版本探测
fn detect_android_version() {
    debug!("--- Android Version ---");
    
    let props = [
        "ro.build.version.sdk",
        "ro.build.version.release",
        "ro.build.version.preview_sdk",
        "ro.build.version.codename",
    ];
    
    for prop in props {
        let value = get_prop(prop);
        debug!("android.{}={}", prop, value);
    }
}

/// 虚拟机检测
fn detect_virtual_machine() {
    debug!("--- Virtual Machine Detection ---");
    
    // 检查 /proc/cpuinfo
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        let lines: Vec<&str> = cpuinfo.lines().take(20).collect();
        for line in lines {
            debug!("cpuinfo: {}", line);
        }
    }
    
    // 检查 /sys/devices/virtual
    if fs::metadata("/sys/devices/virtual").is_ok() {
        debug!("virtual_device=/sys/devices/virtual exists");
    }
    
    // 检查 VPhoneOS 特征
    let model = get_prop("ro.product.model");
    if model.contains("vphone") || model.contains("VPhone") || model.contains("titan") {
        debug!("virtual_machine=true (detected from model: {})", model);
    }
    
    // 检查 sharefs（VPhoneOS 特征）
    if fs::metadata("/share").is_ok() {
        debug!("sharefs=true (VPhoneOS detected)");
    }
    
    // 检查内核 cmdline 中的虚拟机特征
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        if cmdline.contains("vphone") || cmdline.contains("titan") {
            debug!("virtual_machine=true (detected from cmdline)");
        }
    }
}

/// Namespace 信息探测
fn detect_namespace_info() {
    debug!("--- Namespace Info ---");
    
    // mount namespace
    if let Ok(mnt_ns) = fs::read_link("/proc/self/ns/mnt") {
        debug!("namespace.mnt={}", mnt_ns.to_string_lossy());
    }
    
    // pid namespace
    if let Ok(pid_ns) = fs::read_link("/proc/self/ns/pid") {
        debug!("namespace.pid={}", pid_ns.to_string_lossy());
    }
    
    // net namespace
    if let Ok(net_ns) = fs::read_link("/proc/self/ns/net") {
        debug!("namespace.net={}", net_ns.to_string_lossy());
    }
    
    // user namespace
    if let Ok(user_ns) = fs::read_link("/proc/self/ns/user") {
        debug!("namespace.user={}", user_ns.to_string_lossy());
    }
    
    // uts namespace
    if let Ok(uts_ns) = fs::read_link("/proc/self/ns/uts") {
        debug!("namespace.uts={}", uts_ns.to_string_lossy());
    }
    
    // cgroup namespace
    if let Ok(cgroup_ns) = fs::read_link("/proc/self/ns/cgroup") {
        debug!("namespace.cgroup={}", cgroup_ns.to_string_lossy());
    }
}

/// 挂载点信息探测
fn detect_mount_info() {
    debug!("--- Mount Info ---");
    
    // /proc/mounts
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        let count = mounts.lines().count();
        debug!("proc.mounts.entries={}", count);
    }
    
    // /proc/self/mountinfo 行数
    let info_count = parse_mount_info("self").len();
    debug!("mountinfo.count={}", info_count);
}

/// 文件系统信息探测
fn detect_filesystem_info() {
    debug!("--- Filesystem Info ---");
    
    // / 文件系统类型
    if let Ok(stat) = nix::sys::statfs::statfs(cstr!("/")) {
        debug!("filesystem.root.type={:?}", stat.filesystem_type());
    }
    
    // /proc/self/mountinfo 完整内容
    debug!("mountinfo.content:");
    for info in parse_mount_info("self") {
        debug!("  {} -> {} ({}, {}, device={}:{})", 
            info.source, info.target, info.fs_type, info.fs_option,
            libc::major(info.device as libc::dev_t), libc::minor(info.device as libc::dev_t));
    }
}

/// 块设备信息探测
fn detect_block_devices() {
    debug!("--- Block Devices ---");
    
    // 列出 /dev/block
    if let Ok(entries) = fs::read_dir("/dev/block") {
        for entry in entries {
            if let Ok(entry) = entry {
                debug!("block.device={}", entry.path().display());
            }
        }
    }
    
    // 列出 /dev/block/by-name（如果存在）
    if let Ok(entries) = fs::read_dir("/dev/block/by-name") {
        for entry in entries {
            if let Ok(entry) = entry {
                debug!("block.by-name={}", entry.path().display());
            }
        }
    }
}

/// 关键目录检查
fn detect_critical_directories() {
    debug!("--- Critical Directories ---");
    
    let dirs = [
        "/data", "/sdcard", "/system", "/vendor", "/apex",
        "/sbin", "/proc", "/sys", "/dev", "/mnt",
        "/data/adb", "/data/adb/magisk", "/data/adb/modules",
        "/first_stage_ramdisk", "/second_stage_resources",
    ];
    
    for dir in dirs {
        let exists = fs::metadata(dir).is_ok();
        debug!("directory.{}={}", dir, exists);
    }
    
    // 检查 /sdcard 是否是符号链接
    if let Ok(link) = fs::read_link("/sdcard") {
        debug!("sdcard.symlink={}", link.display());
    }
}

/// 加密状态探测
fn detect_encryption_status() {
    debug!("--- Encryption Status ---");
    
    let props = [
        "ro.crypto.state",
        "ro.crypto.type",
        "ro.crypto.metadata.enabled",
    ];
    
    for prop in props {
        let value = get_prop(prop);
        debug!("encryption.{}={}", prop, value);
    }
}

/// SELinux 状态探测
fn detect_selinux_status() {
    debug!("--- SELinux Status ---");
    
    // 检查 SELinux enforce 状态
    if let Ok(enforce) = fs::read_to_string("/sys/fs/selinux/enforce") {
        debug!("selinux.enforce={}", enforce.trim());
    }
    
    // 检查 SELinux policy 版本
    if let Ok(policyvers) = fs::read_to_string("/sys/fs/selinux/policyvers") {
        debug!("selinux.policyvers={}", policyvers.trim());
    }
    
    // 检查 SELinux 是否存在
    let selinux_exists = fs::metadata("/sys/fs/selinux").is_ok();
    debug!("selinux.mounted={}", selinux_exists);
}

/// AB 分区和 SLOT 信息探测
fn detect_partition_info() {
    debug!("--- Partition Info ---");
    
    let props = [
        "ro.build.ab_update",
        "ro.boot.slot_suffix",
        "ro.boot.dynamic_partitions",
        "ro.virtual_ab.enabled",
    ];
    
    for prop in props {
        let value = get_prop(prop);
        debug!("partition.{}={}", prop, value);
    }
}

/// boot mode 探测
fn detect_boot_mode() {
    debug!("--- Boot Mode ---");
    
    let props = [
        "ro.bootmode",
        "sys.boot_completed",
        "ro.boot.bootreason",
        "ro.boot.hardware",
    ];
    
    for prop in props {
        let value = get_prop(prop);
        debug!("boot.{}={}", prop, value);
    }
}

/// Magisk 环境探测
fn detect_magisk_environment() {
    debug!("--- Magisk Environment ---");
    
    // 检查 Magisk 二进制文件
    let files = [
        "/sbin/magisk", "/sbin/su", "/sbin/magiskinit",
        "/sbin/magiskpolicy", "/sbin/resetprop",
    ];
    
    for file in files {
        let exists = fs::metadata(file).is_ok();
        debug!("magisk.file.{}={}", file, exists);
    }
    
    // 检查 Magisk 配置目录
    let config_dirs = [
        "/sbin/.magisk",
        "/sbin/.magisk/device",
        "/sbin/.magisk/block",
        "/sbin/.magisk/preinit",
        "/sbin/.magisk/mirror",
    ];
    
    for dir in config_dirs {
        let exists = fs::metadata(dir).is_ok();
        debug!("magisk.dir.{}={}", dir, exists);
    }
    
    // 读取 Magisk 配置文件
    if let Ok(content) = fs::read_to_string("/sbin/.magisk/config") {
        debug!("magisk.config.content:");
        for line in content.lines() {
            debug!("  {}", line);
        }
    }
}

/// 获取系统属性
fn get_prop(prop: &str) -> String {
    // 尝试从 /proc/cmdline 获取
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        for part in cmdline.split_whitespace() {
            if let Some(value) = part.strip_prefix(&format!("{}=", prop)) {
                return value.to_string();
            }
        }
    }
    
    // 尝试从 build.prop 文件获取
    // 注意：在第一阶段 /system 可能无法访问
    let prop_files = [
        "/system/build.prop",
        "/vendor/build.prop",
        "/product/build.prop",
        "/system_ext/build.prop",
    ];
    
    for file in &prop_files {
        if let Ok(content) = fs::read_to_string(file) {
            for line in content.lines() {
                let line = line.trim();
                // 跳过注释和空行
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                // 解析 key=value 格式
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == prop {
                        return value.trim().to_string();
                    }
                }
            }
        }
    }
    
    String::new()
}