use crate::ffi::MagiskInit;
use base::nix::fcntl::OFlag;
use base::{LoggedResult, MappedFile, MutBytesExt, ResultExt, cstr, debug, error, info};
use std::io::Write;

pub(crate) fn hexpatch_init_for_second_stage(writable: bool) -> bool {
    info!("[hexpatch] hexpatch_init_for_second_stage(writable={})", writable);
    
    // 检查 /init 文件是否存在
    let init_path = cstr!("/init");
    if !init_path.exists() {
        error!("[hexpatch] /init does not exist!");
        return false;
    }
    
    let init = if writable {
        MappedFile::open_rw(cstr!("/init"))
    } else {
        MappedFile::open(cstr!("/init"))
    };

    let Ok(mut init) = init else {
        error!("[hexpatch] Failed to open /init");
        return false;
    };
    
    debug!("[hexpatch] /init opened, mapped size={}", init.as_ref().len());
    
    // 打印内存内容前64字节（仅 debug 模式）
    let preview_len = std::cmp::min(64, init.as_ref().len());
    let preview = &init.as_ref()[..preview_len];
    debug!("[hexpatch] /init content preview (first {} bytes): {:02x?}", preview_len, preview);

    // Redirect original init to magiskinit
    let from = "/system/bin/init";
    let to = "/data/magiskinit";
    debug!("[hexpatch] Searching for '{}' to replace with '{}'", from, to);
    
    let v = init.patch(from.as_bytes(), to.as_bytes());
    
    // 打印 patch 结果（关键信息，使用 info/error）
    if v.is_empty() {
        error!("[hexpatch] PATCH FAILED! No match for '{}' in /init", from);
        return false;
    } else {
        info!("[hexpatch] PATCH OK! Found {} match(es)", v.len());
    }
    
    #[allow(unused_variables)]
    for off in &v {
        debug!("[hexpatch] Patch @ {:#010X} [{}] -> [{}]", off, from, to);
    }

    if !writable {
        // If we cannot directly modify /init, we need to bind mount a replacement on top of it
        let src = cstr!("/init");
        let dest = cstr!("/data/init");
        let _ = || -> LoggedResult<()> {
            {
                let mut fd = dest.create(OFlag::O_CREAT | OFlag::O_WRONLY, 0)?;
                fd.write_all(init.as_ref())?;
            }
            let attr = src.follow_link().get_attr()?;
            dest.set_attr(&attr)?;
            dest.bind_mount_to(src, false)?;
            Ok(())
        }();
    }
    
    true
}

impl MagiskInit {
    pub(crate) fn hijack_init_with_switch_root(&self) {
        // We make use of original init's `SwitchRoot` to help us bind mount
        // magiskinit to /system/bin/init to hijack second stage init.
        //
        // Two important assumption about 2SI:
        // - The second stage init is always /system/bin/init
        // - After `SwitchRoot`, /sdcard is always a symlink to `/storage/self/primary`.
        //
        // `SwitchRoot` will perform the following:
        // - Recursive move all mounts under `/` to `/system`
        // - chroot to `/system`
        //
        // The trick here is that in Magisk's first stage init, we can mount magiskinit to /sdcard,
        // and create a symlink at /storage/self/primary pointing to /system/system/bin/init.
        //
        // During init's `SwitchRoot`, it will mount move /sdcard (which is magiskinit)
        // to /system/sdcard, which is a symlink to /storage/self/primary, which is a
        // symlink to /system/system/bin/init, which will eventually become /system/bin/init after
        // chroot to /system. The effective result is that we coerce the original init into bind
        // mounting magiskinit to /system/bin/init, successfully hijacking the second stage init.
        //
        // An edge case is that some devices (like meizu) use 2SI but does not switch root.
        // In that case, they must already have a /sdcard in ramfs, thus we can check if
        // /sdcard exists and fallback to using hexpatch.

        if self.config.force_normal_boot {
            cstr!("/first_stage_ramdisk/storage/self")
                .mkdirs(0o755)
                .log_ok();
            cstr!("/first_stage_ramdisk/storage/self/primary")
                .create_symlink_to(cstr!("/system/system/bin/init"))
                .log_ok();
            debug!("Symlink /first_stage_ramdisk/storage/self/primary -> /system/system/bin/init");
            cstr!("/first_stage_ramdisk/sdcard")
                .create(OFlag::O_RDONLY | OFlag::O_CREAT | OFlag::O_CLOEXEC, 0)
                .log_ok();
        } else {
            cstr!("/storage/self").mkdirs(0o755).log_ok();
            cstr!("/storage/self/primary")
                .create_symlink_to(cstr!("/system/system/bin/init"))
                .log_ok();
            debug!("Symlink /storage/self/primary -> /system/system/bin/init");
        }
        cstr!("/init").rename_to(cstr!("/sdcard")).log_ok();

        // First try to mount magiskinit from rootfs to workaround Samsung RKP
        if cstr!("/sdcard")
            .bind_mount_to(cstr!("/sdcard"), false)
            .is_ok()
        {
            debug!("Bind mount /sdcard -> /sdcard");
        } else {
            // Binding mounting from rootfs is not supported before Linux 3.12
            cstr!("/data/magiskinit")
                .bind_mount_to(cstr!("/sdcard"), false)
                .log_ok();
            debug!("Bind mount /data/magiskinit -> /sdcard");
        }
    }
}
