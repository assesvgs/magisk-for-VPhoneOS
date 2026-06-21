// 外部 crate
use base::const_format::concatcp;
use base::{BufReadExt, FsPathBuilder, ResultExt, cstr, debug, error, info, warn, parse_mount_info};
use bitflags::bitflags;
use nix::fcntl::OFlag;

// 内部模块
use crate::consts::{APP_PACKAGE_NAME, BBPATH, DATABIN, MODULEROOT, SECURE_DIR};
use crate::daemon::MagiskD;
use crate::ffi::{
    DbEntryKey, RequestCode, check_key_combo, enable_mount_su, exec_common_scripts,
    exec_module_scripts, get_magisk_tmp, initialize_denylist,
};
use crate::logging::setup_logfile;
use crate::module::disable_modules;
use crate::mount::{clean_mounts, setup_preinit_dir};
use crate::resetprop::get_prop;
use crate::selinux::{restorecon, setfilecon};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

bitflags! {
    #[derive(Default)]
    pub struct BootState : u32 {
        const PostFsDataDone = 1 << 0;
        const LateStartDone = 1 << 1;
        const BootComplete = 1 << 2;
        const SafeMode = 1 << 3;
    }
}

/// VPhoneOS sdcard 链路修复
///
/// 在 VPhoneOS 容器中，vold 可能不挂载 sdcardfs，导致 /storage/emulated/0 不存在
/// 和 /sdcard 链路断裂。此函数在 VPhoneOS 环境中重建链路。
///
/// 注意：不动 /sdcard（在 RO ext4 上），在 tmpfs rw 的 /storage/ 下重建：
///   1. mkdir /storage/emulated/0, chown root:sdcard_rw, chmod 771
///   2. mount --bind /data/media/0 /storage/emulated/0
///   3. restorecon
///   4. MS_REC|MS_SHARED on /storage
///   5. ln -sf /storage/emulated/0 /storage/self/primary
///
/// 在各阶段（post_fs_data, late_start, boot_complete）均尝试，幂等。
fn bind_sdcard_in_vphoneos(stage: &str) {
    debug!("{}: bind_sdcard_in_vphoneos entered", stage);

    // 仅 VPhoneOS 环境触发
    if !cstr!("/share").exists() {
        return;
    }
    debug!("{}: VPhoneOS detected, checking /storage/emulated/0", stage);

    let emulated_ok = cstr!("/storage/emulated/0").follow_link().exists();
    debug!("{}: /storage/emulated/0 accessible={}", stage, emulated_ok);
    if emulated_ok {
        debug!("{}: already accessible, no fix needed", stage);
        return;
    }

    if !cstr!("/data/media/0").follow_link().exists() {
        error!("{}: /data/media/0 not available, cannot fix sdcard", stage);
        return;
    }

    // 从 /data/media/0 动态获取 GID（评审 3.3）
    // 使用 Magisk 的 get_attr() 替代 std::os::unix::MetadataExt（跨平台兼容）
    let gid = match cstr!("/data/media/0").follow_link().get_attr() {
        Ok(attr) => {
            let g = attr.st.st_gid as u32;
            debug!("{}: /data/media/0 st_gid={}", stage, g);
            g
        }
        Err(_) => {
            debug!("{}: stat /data/media/0 failed, using fallback gid=1015", stage);
            1015
        }
    };

    // 1. 创建 /storage/emulated/0 目录
    debug!("{}: attempting mkdir /storage/emulated/0", stage);
    if let Err(e) = std::fs::create_dir_all("/storage/emulated/0") {
        error!("{}: mkdir /storage/emulated/0 failed: {}", stage, e);
        return;
    }
    debug!("{}: mkdir /storage/emulated/0 success", stage);

    // 2. 设置所有者 root:sdcard_rw，权限 771
    //    使用 Magisk 的 chmod() 替代 std::Permissions::from_mode（跨平台兼容）
    {
        let c_res = std::os::unix::fs::chown("/storage/emulated/0", Some(0), Some(gid));
        let p_res = cstr!("/storage/emulated/0").follow_link().chmod(0o771).is_ok();
        debug!("{}: chown(root,{}) ok={}, chmod(771) ok={}", stage, gid, c_res.is_ok(), p_res);
    }

    // 3. bind mount /data/media/0 → /storage/emulated/0
    debug!("{}: bind_mount /data/media/0 -> /storage/emulated/0", stage);
    if let Err(e) = cstr!("/data/media/0").bind_mount_to(cstr!("/storage/emulated/0"), false) {
        error!("{}: bind_mount /data/media/0 -> /storage/emulated/0 failed: {}", stage, e);
        return;
    }
    debug!("{}: bind_mount success", stage);

    // 4. 恢复 SELinux context（评审 3.1）
    let se_ok = setfilecon(cstr!("/storage/emulated/0"), cstr!("u:object_r:sdcard_external:s0"));
    debug!("{}: setfilecon sdcard_external ok={}", stage, se_ok);

    // 5. 设置 MS_SHARED 传播（评审 2.3）
    //    对整个 /storage 递归设置，确保 Zygisk unshare(CLONE_NEWNS) 后
    //    新 namespace 能继承这些挂载
    let shared_ok = cstr!("/storage").set_mount_shared(true).is_ok();
    debug!("{}: set_mount_shared /storage ok={}", stage, shared_ok);

    // 6. 修复 /storage/self/primary 符号链接
    //    /storage/self/primary 位于 tmpfs（rw），可安全删除
    {
        let rm_ok = std::fs::remove_file("/storage/self/primary").is_ok();
        debug!("{}: remove_file /storage/self/primary ok={}", stage, rm_ok);
    }
    match std::os::unix::fs::symlink("/storage/emulated/0", "/storage/self/primary") {
        Ok(()) => debug!("{}: symlink /storage/self/primary -> /storage/emulated/0 success", stage),
        Err(e) => error!("{}: symlink /storage/self/primary -> /storage/emulated/0 failed: {}", stage, e),
    }

    // 7. 验证 /sdcard 符号链接目标（评审 3.2）
    match std::fs::read_link("/sdcard") {
        Ok(target) => {
            let target = target.to_string_lossy();
            let expected = "/storage/self/primary";
            if target == expected {
                debug!("{}: /sdcard -> {} (expected)", stage, expected);
            } else {
                warn!("{}: /sdcard points to '{}', expected '{}' (cannot fix, on RO fs)", stage, target, expected);
            }
        }
        Err(e) => {
            warn!("{}: /sdcard read_link failed: {}", stage, e);
        }
    }

    // 8. 最终验证（评审 4.2）：成功/失败各一条日志
    if cstr!("/storage/emulated/0").follow_link().exists() {
        let size = std::fs::metadata("/data/media/0").map(|m| m.len()).unwrap_or(0);
        info!("{}: storage fix applied, /storage/emulated/0 -> /data/media/0 (size={})", stage, size);
        debug!("{}: /storage/self/primary accessible={}", stage, cstr!("/storage/self/primary").follow_link().exists());
        debug!("{}: /sdcard accessible={}", stage, cstr!("/sdcard").follow_link().exists());
    } else {
        error!("{}: storage fix FAILED - /storage/emulated/0 still inaccessible", stage);
    }
}

impl MagiskD {
    fn setup_magisk_env(&self) -> bool {
        info!("* Initializing Magisk environment");

        let mut buf = cstr::buf::default();

        let app_bin_dir = buf
            .append_path(self.app_data_dir())
            .append_path("0")
            .append_path(APP_PACKAGE_NAME)
            .append_path("install");

        // Alternative binaries paths
        let alt_bin_dirs = &[
            cstr!("/cache/data_adb/magisk"),
            cstr!("/data/magisk"),
            app_bin_dir,
        ];
        for dir in alt_bin_dirs {
            if dir.exists() {
                cstr!(DATABIN).remove_all().ok();
                dir.copy_to(cstr!(DATABIN)).ok();
                dir.remove_all().ok();
            }
        }
        cstr!("/cache/data_adb").remove_all().ok();

        // Directories in /data/adb
        cstr!(SECURE_DIR).follow_link().chmod(0o700).log_ok();
        cstr!(DATABIN).mkdir(0o755).log_ok();
        cstr!(MODULEROOT).mkdir(0o755).log_ok();
        cstr!(concatcp!(SECURE_DIR, "/post-fs-data.d"))
            .mkdir(0o755)
            .log_ok();
        cstr!(concatcp!(SECURE_DIR, "/service.d"))
            .mkdir(0o755)
            .log_ok();
        restorecon();

        let busybox = cstr!(concatcp!(DATABIN, "/busybox"));
        if !busybox.exists() {
            return false;
        }

        let tmp_bb = buf.append_path(get_magisk_tmp()).append_path(BBPATH);
        tmp_bb.mkdirs(0o755).ok();
        tmp_bb.append_path("busybox");
        busybox.copy_to(tmp_bb).ok();
        tmp_bb.follow_link().chmod(0o755).log_ok();

        // Install busybox applets
        Command::new(&tmp_bb)
            .arg("--install")
            .arg("-s")
            .arg(tmp_bb.parent_dir().unwrap_or_default())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .log_ok();

        // magisk32 and magiskpolicy are not installed into ramdisk and has to be copied
        // from data to magisk tmp
        let magisk32 = cstr!(concatcp!(DATABIN, "/magisk32"));
        if magisk32.exists() {
            let tmp = buf.append_path(get_magisk_tmp()).append_path("magisk32");
            magisk32.copy_to(tmp).log_ok();
        }
        let magiskpolicy = cstr!(concatcp!(DATABIN, "/magiskpolicy"));
        if magiskpolicy.exists() {
            let tmp = buf
                .append_path(get_magisk_tmp())
                .append_path("magiskpolicy");
            magiskpolicy.copy_to(tmp).log_ok();
        }

        true
    }

    fn post_fs_data(&self) -> bool {
        setup_logfile();
        info!("** post-fs-data mode running");

        self.preserve_stub_apk();

        // Check secure dir
        debug!("post_fs_data: checking secure dir");
        let secure_dir = cstr!(SECURE_DIR);
        if !secure_dir.exists() {
            if self.sdk_int < 24 {
                secure_dir.mkdir(0o700).log_ok();
            } else {
                error!("* {} is not present, abort", SECURE_DIR);
                return true;
            }
        }

        self.prune_su_access();

        info!("post_fs_data: setting up Magisk environment");
        if !self.setup_magisk_env() {
            error!("* Magisk environment incomplete, abort");
            return true;
        }
        info!("post_fs_data: setup_magisk_env done");

        // Check safe mode
        info!("post_fs_data: checking safe mode");
        let boot_cnt = self.get_db_setting(DbEntryKey::BootloopCount);
        self.set_db_setting(DbEntryKey::BootloopCount, boot_cnt + 1)
            .log()
            .ok();
        let safe_mode = boot_cnt >= 2
            || get_prop(cstr!("persist.sys.safemode")) == "1"
            || get_prop(cstr!("ro.sys.safemode")) == "1"
            || check_key_combo();
        info!("post_fs_data: safe_mode={}", safe_mode);

        if safe_mode {
            info!("* Safe mode triggered");
            // Disable all modules and zygisk so next boot will be clean
            disable_modules();
            self.set_db_setting(DbEntryKey::ZygiskConfig, 0).log_ok();
            return true;
        }

        info!("post_fs_data: executing post-fs-data scripts");
        exec_common_scripts(cstr!("post-fs-data"));
        info!("post_fs_data: post-fs-data scripts done");
        self.zygisk_enabled.store(
            self.get_db_setting(DbEntryKey::ZygiskConfig) != 0,
            Ordering::Release,
        );
        initialize_denylist();
        info!("post_fs_data: handling modules");
        self.handle_modules();
        info!("post_fs_data: handle_modules done");
        info!("post_fs_data: clean_mounts");
        clean_mounts();

        // VPhoneOS sdcard 修复（post_fs_data 阶段首次尝试）
        bind_sdcard_in_vphoneos("post_fs_data");

        // [诊断] 检查 sdcard 状态
        // 注意：使用 debug!() 而不是 info!()，因为 info!() 输出到 stdout，
        // 会被 boot_patch.sh 的 $(./magisk --preinit-device) 捕获，污染 PREINITDEVICE 变量
        debug!("post_fs_data: /sdcard exists={}", cstr!("/sdcard").exists());
        debug!("post_fs_data: /storage/self/primary exists={}", cstr!("/storage/self/primary").exists());
        debug!("post_fs_data: init.svc.vold={}", get_prop(cstr!("init.svc.vold")));

        // 检查 sdcardfs 挂载
        let sdcardfs_mounts: Vec<_> = parse_mount_info("self")
            .into_iter()
            .filter(|info| info.fs_type == "sdcardfs")
            .collect();
        if sdcardfs_mounts.is_empty() {
            debug!("post_fs_data: sdcardfs NOT found in mountinfo");
        } else {
            for info in &sdcardfs_mounts {
                debug!("post_fs_data: sdcardfs mounted at {}", info.target);
            }
        }
        
        info!("post_fs_data: done");

        false
    }

    fn late_start(&self) {
        setup_logfile();
        info!("** late_start service mode running");

        debug!("late_start: executing common scripts");
        exec_common_scripts(cstr!("service"));
        if let Some(module_list) = self.module_list.get() {
            info!("late_start: executing module scripts");
            exec_module_scripts(cstr!("service"), module_list);
        }
        
        // [诊断] 检查 sdcard 状态
        // 注意：使用 debug!() 而不是 info!()，因为 info!() 输出到 stdout，
        // 会被 boot_patch.sh 的 $(./magisk --preinit-device) 捕获，污染 PREINITDEVICE 变量
        debug!("late_start: /sdcard exists={}", cstr!("/sdcard").exists());
        debug!("late_start: /storage/self/primary exists={}", cstr!("/storage/self/primary").exists());
        debug!("late_start: init.svc.vold={}", get_prop(cstr!("init.svc.vold")));

        // 检查 sdcardfs 挂载
        let sdcardfs_mounts: Vec<_> = parse_mount_info("self")
            .into_iter()
            .filter(|info| info.fs_type == "sdcardfs")
            .collect();
        if sdcardfs_mounts.is_empty() {
            debug!("late_start: sdcardfs NOT found in mountinfo");
        } else {
            for info in &sdcardfs_mounts {
                debug!("late_start: sdcardfs mounted at {}", info.target);
            }
        }

        // VPhoneOS sdcard 修复（延迟到 late_start，此时 /data 应已完全挂载）
        bind_sdcard_in_vphoneos("late_start");
        
        info!("late_start: done");
    }

    fn boot_complete(&self) {
        setup_logfile();
        info!("** boot-complete triggered");

        // Reset the bootloop counter once we have boot-complete
        debug!("boot_complete: resetting bootloop counter");
        self.set_db_setting(DbEntryKey::BootloopCount, 0).log_ok();

        // Mount MagiskSU (Kitsune Mask feature)
        info!("boot_complete: enabling mount su");
        enable_mount_su();

        // At this point it's safe to create the folder
        info!("boot_complete: ensuring secure dir");
        let secure_dir = cstr!(SECURE_DIR);
        if !secure_dir.exists() {
            secure_dir.mkdir(0o700).log_ok();
        }

        info!("boot_complete: calling setup_preinit_dir()");
        setup_preinit_dir();
        info!("boot_complete: setup_preinit_dir done");
        info!("boot_complete: ensuring manager");
        self.ensure_manager();
        if self.zygisk_enabled.load(Ordering::Relaxed) {
            info!("boot_complete: resetting zygisk");
            self.zygisk.lock().reset(true);
        }
        // VPhoneOS sdcard 修复（boot_complete 阶段，/data 一定已挂载）
        bind_sdcard_in_vphoneos("boot_complete");

        info!("boot_complete: done");
    }

    pub fn boot_stage_handler(&self, client: UnixStream, code: RequestCode) {
        // Make sure boot stage execution is always serialized
        let mut state = self.boot_stage_lock.lock();

        match code {
            RequestCode::POST_FS_DATA => {
                info!("boot_stage_handler: POST_FS_DATA");
                if check_data() && !state.contains(BootState::PostFsDataDone) {
                    info!("boot_stage_handler: calling post_fs_data()");
                    if self.post_fs_data() {
                        state.insert(BootState::SafeMode);
                    }
                    state.insert(BootState::PostFsDataDone);
                    info!("boot_stage_handler: post_fs_data done");
                }
            }
            RequestCode::LATE_START => {
                info!("boot_stage_handler: LATE_START");
                drop(client);
                if state.contains(BootState::PostFsDataDone) && !state.contains(BootState::SafeMode)
                {
                    info!("boot_stage_handler: calling late_start()");
                    self.late_start();
                    state.insert(BootState::LateStartDone);
                    info!("boot_stage_handler: late_start done");
                }
            }
            RequestCode::BOOT_COMPLETE => {
                info!("boot_stage_handler: BOOT_COMPLETE");
                drop(client);
                if state.contains(BootState::PostFsDataDone) {
                    state.insert(BootState::BootComplete);
                    info!("boot_stage_handler: calling boot_complete()");
                    self.boot_complete();
                    info!("boot_stage_handler: boot_complete done");
                }
            }
            _ => {}
        }
    }
}

fn check_data() -> bool {
    if let Ok(file) = cstr!("/proc/mounts").open(OFlag::O_RDONLY | OFlag::O_CLOEXEC) {
        let mut mnt = false;
        BufReader::new(file).for_each_line(|line| {
            if line.contains(" /data ") && !line.contains("tmpfs") {
                mnt = true;
                return false;
            }
            true
        });
        if !mnt {
            return false;
        }
        let crypto = get_prop(cstr!("ro.crypto.state"));
        return if !crypto.is_empty() {
            if crypto != "encrypted" {
                // Unencrypted, we can directly access data
                true
            } else {
                // Encrypted, check whether vold is started
                !get_prop(cstr!("init.svc.vold")).is_empty()
            }
        } else {
            // ro.crypto.state is not set, assume it's unencrypted
            true
        };
    }
    false
}
