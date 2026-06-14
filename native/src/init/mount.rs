use crate::ffi::MagiskInit;
use base::{
    Directory, FsPathBuilder, LibcReturn, LoggedResult, ResultExt, Utf8CStr, cstr, debug, libc,
    nix, parse_mount_info, raw_cstr,
};
use cxx::CxxString;
use nix::mount::MsFlags;
use nix::sys::statfs::{FsType, TMPFS_MAGIC, statfs};
use nix::unistd::{chdir, chroot};
use num_traits::AsPrimitive;
use std::collections::BTreeSet;
use std::ops::Bound::{Excluded, Unbounded};
use std::pin::Pin;

unsafe extern "C" {
    static environ: *const *mut libc::c_char;
}

pub(crate) fn switch_root(path: &Utf8CStr) {
    || -> LoggedResult<()> {
        debug!("Switch root to {}", path);
        let mut mounts = BTreeSet::new();
        let rootfs = Directory::open(cstr!("/"))?;
        for info in parse_mount_info("self") {
            if info.target == "/" || info.target.as_str() == path.as_str() {
                continue;
            }
            if let Some(last_mount) = mounts
                .range::<String, _>((Unbounded, Excluded(&info.target)))
                .last()
                && info.target.starts_with(&format!("{}/", *last_mount))
            {
                continue;
            }

            let mut target = info.target.clone();
            let target = Utf8CStr::from_string(&mut target);
            let new_path = cstr::buf::default()
                .join_path(path)
                .join_path(info.target.trim_start_matches('/'));
            new_path.mkdirs(0o755).ok();
            target.move_mount_to(&new_path)?;
            mounts.insert(info.target);
        }
        chdir(path)?;
        path.move_mount_to(cstr!("/"))?;
        chroot(cstr!("."))?;

        debug!("Cleaning rootfs");
        rootfs.remove_all()?;
        Ok(())
    }()
    .ok();
}

pub(crate) fn is_device_mounted(dev: u64, target: Pin<&mut CxxString>) -> bool {
    for mount in parse_mount_info("self") {
        if mount.root == "/" && mount.device == dev {
            target.push_str(&mount.target);
            return true;
        }
    }
    false
}

/// 从 mountinfo 检测 /data 分区的设备号
/// 用于 fallback：当 preinit_dev 为空时，在 init namespace 中直接检测
/// 参考 27.0 的思路：先找已挂载分区，bind mount 避免直接挂载块设备
pub(crate) fn detect_data_device() -> u64 {
    debug!("detect_data_device: scanning /proc/self/mountinfo for /data partition");
    for info in parse_mount_info("self") {
        if info.target == "/data"
            && info.source.starts_with('/')
            && !info.source.contains("/dm-")
            && (info.fs_type == "ext4" || info.fs_type == "f2fs")
            && info.fs_option.split(',').any(|s| s == "rw")
        {
            debug!(
                "detect_data_device: found /data on {} (fs_type={}, device={})",
                info.source, info.fs_type, info.device
            );
            return info.device;
        }
    }
    debug!("detect_data_device: /data partition not found");
    0
}

const RAMFS_MAGIC: u32 = 0x858458f6;

pub(crate) fn is_rootfs() -> bool {
    if let Ok(s) = statfs(cstr!("/")) {
        s.filesystem_type() == FsType(RAMFS_MAGIC.as_()) || s.filesystem_type() == TMPFS_MAGIC
    } else {
        false
    }
}

impl MagiskInit {
    pub(crate) fn prepare_data(&self) {
        debug!("Setup data tmp");
        cstr!("/data").mkdir(0o755).log_ok();
        nix::mount::mount(
            Some(cstr!("magisk")),
            cstr!("/data"),
            Some(cstr!("tmpfs")),
            MsFlags::empty(),
            Some(cstr!("mode=755")),
        )
        .check_os_err("mount", Some("/data"), Some("tmpfs"))
        .log_ok();

        cstr!("/init").copy_to(cstr!("/data/magiskinit")).ok();
        cstr!("/.backup").copy_to(cstr!("/data/.backup")).ok();
        cstr!("/overlay.d").copy_to(cstr!("/data/overlay.d")).ok();
    }

    pub(crate) fn exec_init(&mut self) {
        for path in self.mount_list.iter_mut().rev() {
            let path = Utf8CStr::from_string(path);
            if path.unmount().log().is_ok() {
                debug!("Unmount [{}]", path);
            }
        }
        unsafe {
            libc::execve(raw_cstr!("/init"), self.argv.cast(), environ.cast())
                .check_err()
                .log_ok();
        }
        std::process::exit(1);
    }
}
