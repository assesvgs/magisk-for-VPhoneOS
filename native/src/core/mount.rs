// 标准库
use std::cmp::Ordering::{Greater, Less};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

// 外部 crate
use base::{
    FsPathBuilder, LibcReturn, LoggedResult, MountInfo, ResultExt, Utf8CStr, Utf8CStrBuf, cstr,
    debug, info, libc, parse_mount_info, warn,
};
use libc::{c_uint, dev_t, major};
#[cfg(debug_assertions)]
use libc::minor;
use nix::mount::MsFlags;
use nix::sys::stat::{Mode, SFlag, mknod};
use num_traits::AsPrimitive;

// 内部模块
use crate::consts::{MODULEMNT, MODULEROOT, PREINITDEV, PREINITMIRR, WORKERDIR};
use crate::ffi::{get_magisk_tmp, resolve_preinit_dir};
use crate::resetprop::get_prop;

// Linux allocated devices: 240-254 are reserved for LOCAL/EXPERIMENTAL use.
const DYNAMIC_MAJOR_MIN: u32 = 240;
const DYNAMIC_MAJOR_MAX: u32 = 254;

pub fn setup_preinit_dir() {
    let magisk_tmp = get_magisk_tmp();
    info!("setup_preinit_dir: start, magisk_tmp={}", magisk_tmp);

    // Mount preinit directory
    let dev_path = cstr::buf::new::<64>()
        .join_path(magisk_tmp)
        .join_path(PREINITDEV);
    debug!("setup_preinit_dir: checking dev_path={}", dev_path);
    if let Ok(attr) = dev_path.get_attr()
        && attr.st.st_mode & libc::S_IFMT as c_uint == libc::S_IFBLK.as_()
    {
        debug!("setup_preinit_dir: dev_path is block device");
        // DO NOT mount the block device directly, as we do not know the flags and configs
        // to properly mount the partition; mounting block devices directly as rw could cause
        // crashes if the filesystem driver is crap (e.g. some broken F2FS drivers).
        // What we do instead is to scan through the current mountinfo and find a pre-existing
        // mount point mounting our desired partition, and then bind mount the target folder.
        let preinit_dev = attr.st.st_rdev;
        debug!("setup_preinit_dir: preinit_dev rdev={}", preinit_dev);
        let mnt_path = cstr::buf::default()
            .join_path(magisk_tmp)
            .join_path(PREINITMIRR);
        debug!("setup_preinit_dir: mnt_path={}", mnt_path);
        for info in parse_mount_info("self") {
            debug!("setup_preinit_dir: checking target={}, device={}, root={}, options={}", 
                info.target, info.device, info.root, info.fs_option);
            if info.root == "/" && info.device == preinit_dev {
                debug!("setup_preinit_dir: found matching device at {}", info.target);
                if !info.fs_option.split(',').any(|s| s == "rw") {
                    debug!("setup_preinit_dir: skip (not rw): {}", info.target);
                    // Only care about rw mounts
                    continue;
                }
                let mut target = info.target;
                let target = Utf8CStr::from_string(&mut target);
                let mut preinit_dir = resolve_preinit_dir(target);
                let preinit_dir = Utf8CStr::from_string(&mut preinit_dir);
                debug!("setup_preinit_dir: trying preinit_dir={}", preinit_dir);
                let r = || -> LoggedResult<()> {
                    preinit_dir.mkdir(0o700)?;
                    mnt_path.mkdirs(0o755)?;
                    mnt_path.remove().ok();
                    mnt_path.create_symlink_to(preinit_dir)?;
                    Ok(())
                }();
                if r.is_ok() {
                    info!("setup_preinit_dir: found preinit dir: {}", preinit_dir);
                    return;
                }
            }
        }
    } else {
        debug!("setup_preinit_dir: dev_path not found or not block device");
    }

    warn!("mount: preinit dir not found");
}

pub fn setup_module_mount() {
    // Bind remount module root to clear nosuid
    let module_mnt = cstr::buf::default()
        .join_path(get_magisk_tmp())
        .join_path(MODULEMNT);
    let _ = || -> LoggedResult<()> {
        module_mnt.mkdir(0o755)?;
        cstr!(MODULEROOT).bind_mount_to(&module_mnt, false)?;
        module_mnt.remount_mount_point_flags(MsFlags::MS_RDONLY)?;
        Ok(())
    }();
}

pub fn clean_mounts() {
    let magisk_tmp = get_magisk_tmp();

    let mut buf = cstr::buf::default();

    let module_mnt = buf.append_path(magisk_tmp).append_path(MODULEMNT);
    module_mnt.unmount().log_ok();
    buf.clear();

    let worker_dir = buf.append_path(magisk_tmp).append_path(WORKERDIR);
    let _ = || -> LoggedResult<()> {
        worker_dir.set_mount_private(true)?;
        worker_dir.unmount()?;
        Ok(())
    }();
}

// when partitions have the same fs type, the order is:
// - data: it has sufficient space and can be safely written
// - cache: size is limited, but still can be safely written
// - klogdump: available on some Smartisan devices and can be safely written
// - metadata: size is limited, and it might cause unexpected behavior if written
// - persist: it's the last resort, as it's dangerous to write to it
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PartId {
    Data,
    Cache,
    Klogdump,
    Metadata,
    Persist,
}

#[derive(Copy, Clone)]
enum EncryptType {
    None,
    Block,
    File,
    Metadata,
}

/// 通过 /sys/dev/block 扫描块设备，查找已知 preinit 分区（回退方案）。
/// 适用于 VPhoneOS 等容器化环境中 shell namespace 无法看到 /data 挂载的场景。
fn find_preinit_device_sysfs() -> String {
    // 被 find_preinit_device() 调用，后者通过 $(./magisk --preinit-device) 捕获 stdout。
    // 禁止 info!()（输出到 stdout），否则会被 $(...) 捕获，污染变量。
    debug!("find_preinit_device_sysfs: start");

    // 已知 preinit 候选分区名（按优先级排列，与 27.0 一致）
    let preinit_targets = ["data", "metadata", "cache", "persist"];

    let mut candidates: Vec<String> = Vec::new();
    let mut scanned_count: u32 = 0;
    let mut no_partname_count: u32 = 0;

    let Ok(entries) = fs::read_dir("/sys/dev/block") else {
        debug!("find_preinit_device_sysfs: cannot read /sys/dev/block");
        return String::new();
    };

    for entry in entries.flatten() {
        scanned_count += 1;
        let devname = entry.file_name().to_string_lossy().to_string();

        // 读取 uevent 获取 PARTNAME
        let uevent_path = format!("/sys/dev/block/{}/uevent", devname);
        let mut partname = String::new();
        if let Ok(content) = fs::read_to_string(&uevent_path) {
            for line in content.lines() {
                if let Some(val) = line.strip_prefix("PARTNAME=") {
                    partname = val.to_string();
                    break;
                }
            }
        } else {
            debug!(
                "find_preinit_device_sysfs: cannot read uevent for {}",
                devname
            );
        }

        // 如果 uevent 中没有 PARTNAME，尝试从 dm/name 读取
        if partname.is_empty() {
            let dm_path = format!("/sys/dev/block/{}/dm/name", devname);
            if let Ok(content) = fs::read_to_string(&dm_path) {
                partname = content.trim().to_string();
            }
        }

        if partname.is_empty() {
            no_partname_count += 1;
            debug!(
                "find_preinit_device_sysfs: skip (no partname): dev={}",
                devname
            );
            continue;
        }

        debug!(
            "find_preinit_device_sysfs: dev={}, partname={}",
            devname, partname
        );

        // 检查是否匹配已知 preinit 候选
        let mut matched = false;
        for target in &preinit_targets {
            if partname.eq_ignore_ascii_case(target) {
                debug!(
                    "find_preinit_device_sysfs: found candidate '{}' on device {}",
                    partname, devname
                );
                candidates.push(partname.clone());
                matched = true;
                break;
            }
        }
        if !matched {
            debug!(
                "find_preinit_device_sysfs: skip (not preinit target): dev={}, partname={}",
                devname, partname
            );
        }
    }

    debug!(
        "find_preinit_device_sysfs: scanned={}, no_partname={}, candidates={:?}",
        scanned_count, no_partname_count, candidates
    );

    if candidates.is_empty() {
        debug!("find_preinit_device_sysfs: no preinit partition found in sysfs");
        return String::new();
    }

    // 优先选择 data 分区（与 27.0 优先级一致）
    let selected = candidates
        .iter()
        .find(|name| name.eq_ignore_ascii_case("data"))
        .unwrap_or(&candidates[0]);

    debug!(
        "find_preinit_device_sysfs: selected partition '{}' from candidates {:?}",
        selected, candidates
    );

    selected.clone()
}

pub fn find_preinit_device() -> String {
    // 此函数被 boot_patch.sh 通过 $(./magisk --preinit-device) 调用，
    // 禁止使用 info!()（输出到 stdout），否则日志会被 $(...) 捕获，
    // 污染 PREINITDEVICE 变量。应使用 debug!()（输出到 stderr）。
    debug!("find_preinit_device: start");

    let encrypt_type = if get_prop(cstr!("ro.crypto.state")) != "encrypted" {
        EncryptType::None
    } else if get_prop(cstr!("ro.crypto.type")) == "block" {
        EncryptType::Block
    } else if get_prop(cstr!("ro.crypto.metadata.enabled")) == "true" {
        EncryptType::Metadata
    } else {
        EncryptType::File
    };
    debug!("find_preinit_device: encrypt_type={}", encrypt_type as u8);

    debug!("find_preinit_device: parsing /proc/self/mountinfo");
    let mut matched_info = parse_mount_info("self")
        .into_iter()
        .filter_map(|info| {
            debug!("find_preinit_device: check target={}, source={}, fs_type={}, device={}:{}",
                info.target, info.source, info.fs_type,
                major(info.device as dev_t), minor(info.device as dev_t));
            if info.root != "/" || !info.source.starts_with('/') || info.source.contains("/dm-") {
                debug!("find_preinit_device: skip (root/source/dm-): target={}", info.target);
                return None;
            }
            match info.fs_type.as_str() {
                "ext4" | "f2fs" => (),
                _ => {
                    debug!("find_preinit_device: skip (fs_type={}): target={}", info.fs_type, info.target);
                    return None;
                },
            }
            if !info.fs_option.split(',').any(|s| s == "rw") {
                debug!("find_preinit_device: skip (not rw): target={}", info.target);
                return None;
            }
            if let Some(path) = Path::new(&info.source).parent() {
                if !path.ends_with("by-name") && !path.ends_with("block") {
                    debug!("find_preinit_device: skip (path={}): target={}", path.display(), info.target);
                    return None;
                }
            } else {
                debug!("find_preinit_device: skip (no parent): target={}", info.target);
                return None;
            }
            // use device major number to filter out device-mapper
            let maj = major(info.device as dev_t) as u32;
            if (DYNAMIC_MAJOR_MIN..=DYNAMIC_MAJOR_MAX).contains(&maj)
                && !info.source.contains("/vd")
                && !info.source.contains("/by-name/")
            {
                debug!("find_preinit_device: skip (device-mapper maj={}): target={}", maj, info.target);
                return None;
            }
            // take data iff it's not encrypted or file-based encrypted without metadata
            // other partitions are always taken
            let result = match info.target.as_str() {
                "/persist" | "/mnt/vendor/persist" => Some((PartId::Persist, info)),
                "/metadata" => Some((PartId::Metadata, info)),
                "/klogdump" => Some((PartId::Klogdump, info)),
                "/cache" => Some((PartId::Cache, info)),
                "/data" => Some((PartId::Data, info))
                    .take_if(|_| matches!(encrypt_type, EncryptType::None | EncryptType::File)),
                _ => None,
            };
            if let Some((_, ref _matched_info)) = result {
                debug!("find_preinit_device: matched target={}", _matched_info.target);
            }
            result
        })
        .collect::<Vec<_>>();

    debug!("find_preinit_device: matched_info count={}", matched_info.len());
    if matched_info.is_empty() {
        debug!("find_preinit_device: mountinfo found nothing, trying sysfs fallback");
        let sysfs_result = find_preinit_device_sysfs();
        if sysfs_result.is_empty() {
            warn!("find_preinit_device: no partition found (mountinfo + sysfs both failed)");
        }
        return sysfs_result;
    }

    let (_, preinit_info, _) = matched_info.select_nth_unstable_by(
        0,
        |(ap, MountInfo { fs_type: at, .. }), (bp, MountInfo { fs_type: bt, .. })| match (
            ap,
            bp,
            at.as_str() == "ext4",
            bt.as_str() == "ext4",
        ) {
            // metadata is not affected by f2fs kernel bug
            (PartId::Metadata, _, _, true) | (_, PartId::Metadata, true, _) => ap.cmp(bp),
            // otherwise, take ext4 f2fs because f2fs has a kernel bug that causes kernel panic
            (_, _, true, false) => Less,
            (_, _, false, true) => Greater,
            // if both has the same fs type, compare the mount point
            _ => ap.cmp(bp),
        },
    );
    let info = &preinit_info.1;
    debug!("find_preinit_device: selected target={}, source={}, fs_type={}", info.target, info.source, info.fs_type);
    let mut target = info.target.clone();
    let mut preinit_dir = resolve_preinit_dir(Utf8CStr::from_string(&mut target));
    debug!("find_preinit_device: preinit_dir={}", preinit_dir);
    if unsafe { libc::getuid() } == 0
        && let Ok(tmp) = std::env::var("MAGISKTMP")
        && !tmp.is_empty()
    {
        debug!("find_preinit_device: MAGISKTMP={}", tmp);
        let mut buf = cstr::buf::default();
        let mirror_dir = buf.append_path(&tmp).append_path(PREINITMIRR);
        let preinit_dir = Utf8CStr::from_string(&mut preinit_dir);
        let _ = || -> LoggedResult<()> {
            preinit_dir.mkdirs(0o700)?;
            mirror_dir.mkdirs(0o755)?;
            mirror_dir.unmount().ok();
            mirror_dir.remove().ok();
            mirror_dir.create_symlink_to(preinit_dir)?;
            Ok(())
        }();
        if std::env::var_os("MAKEDEV").is_some() {
            buf.clear();
            let dev_path = buf.append_path(&tmp).append_path(PREINITDEV);
            mknod(
                dev_path.as_utf8_cstr(),
                SFlag::S_IFBLK,
                Mode::from_bits_truncate(0o600),
                info.device as dev_t,
            )
            .check_os_err("mknod", Some(dev_path), None)
            .log_ok();
        }
    }
    let result = Path::new(&info.source)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    debug!("find_preinit_device: result='{}'", result);
    result
}

// revert_unmount is now implemented in C++ (deny/revert.cpp)
// for more complete unmounting support from Kitsune Mask
