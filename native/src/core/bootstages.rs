// 外部 crate
use base::const_format::concatcp;
use base::{BufReadExt, FsPathBuilder, ResultExt, cstr, debug, error, info, libc, parse_mount_info, warn};
use bitflags::bitflags;
use nix::fcntl::OFlag;

// 内部模块
use crate::consts::{APP_PACKAGE_NAME, BBPATH, DATABIN, MODULEROOT, SECURE_DIR, WORKERDIR};
use crate::daemon::MagiskD;
use crate::ffi::{
    DbEntryKey, RequestCode, check_key_combo, enable_mount_su, exec_common_scripts,
    exec_module_scripts, get_magisk_tmp, initialize_denylist,
};
use crate::logging::setup_logfile;
use crate::module::disable_modules;
use crate::mount::{clean_mounts, setup_preinit_dir};
use crate::resetprop::{get_prop, set_prop};
use crate::selinux::restorecon;
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

        // 将所有二进制从 data 分区复制到 magisk tmp
        // 顺序：先全部复制，再检查可用性（避免检查时文件尚未就绪的误报）
        let magisk32_src = cstr!(concatcp!(DATABIN, "/magisk32"));
        if magisk32_src.exists() {
            let tmp = buf.append_path(get_magisk_tmp()).append_path("magisk32");
            magisk32_src.copy_to(tmp).log_ok();
        }
        let magisk64_src = cstr!(concatcp!(DATABIN, "/magisk64"));
        if magisk64_src.exists() {
            let tmp = buf.append_path(get_magisk_tmp()).append_path("magisk64");
            magisk64_src.copy_to(tmp).log_ok();
        }
        let magisk_src = cstr!(concatcp!(DATABIN, "/magisk"));
        if magisk_src.exists() {
            let tmp = buf.append_path(get_magisk_tmp()).append_path("magisk");
            magisk_src.copy_to(tmp).log_ok();
        }
        let magiskpolicy_src = cstr!(concatcp!(DATABIN, "/magiskpolicy"));
        if magiskpolicy_src.exists() {
            let tmp = buf
                .append_path(get_magisk_tmp())
                .append_path("magiskpolicy");
            magiskpolicy_src.copy_to(tmp).log_ok();
        }
        let zygisk_inject_src = cstr!(concatcp!(DATABIN, "/zygisk_inject"));
        if zygisk_inject_src.exists() {
            let tmp = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
            zygisk_inject_src.copy_to(tmp).log_ok();
        }
        let zygisk_inject32_src = cstr!(concatcp!(DATABIN, "/zygisk_inject32"));
        if zygisk_inject32_src.exists() {
            let tmp32 = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject32");
            zygisk_inject32_src.copy_to(tmp32).log_ok();
        }
        // 以上全部复制完成后，检查关键 tracer/注入文件是否到位
        if self.get_db_setting(DbEntryKey::ZygiskConfig) != 0 {
            let tmp_magisk = buf.append_path(get_magisk_tmp()).append_path("magisk");
            if !tmp_magisk.exists() {
                warn!("zygisk: magisk tracer absent in {}, 64-bit Zygisk will fail",
                      get_magisk_tmp());
            }
            let tmp32 = buf.append_path(get_magisk_tmp()).append_path("magisk32");
            if !tmp32.exists() {
                warn!("zygisk: magisk32 tracer absent in {}, 32-bit Zygisk will fail",
                      get_magisk_tmp());
            }
            let tmp_inject = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
            if !tmp_inject.exists() {
                warn!("zygisk: zygisk_inject absent in {}, 64-bit injection target missing",
                      get_magisk_tmp());
            }
            let tmp_inject32 = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject32");
            if !tmp_inject32.exists() {
                warn!("zygisk: zygisk_inject32 absent in {}, 32-bit injection target missing",
                      get_magisk_tmp());
            }
        }

        true
    }

    fn post_fs_data(&self) -> bool {
        setup_logfile();
        diag(1, "post_fs_data");
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

        debug!("post_fs_data: setting up Magisk environment");
        if !self.setup_magisk_env() {
            error!("* Magisk environment incomplete, abort");
            return true;
        }
        debug!("post_fs_data: setup_magisk_env done");

        // Check safe mode
        debug!("post_fs_data: checking safe mode");
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
            // Disable all modules so next boot will be clean
            disable_modules();
            return true;
        }

        debug!("post_fs_data: executing post-fs-data scripts");
        exec_common_scripts(cstr!("post-fs-data"));
        debug!("post_fs_data: post-fs-data scripts done");

        // Read DB and store zygisk_enabled BEFORE initialize_denylist
        // so that enable_deny() checks zygisk_enabled and skips denylist monitor
        self.zygisk_enabled.store(
            self.get_db_setting(DbEntryKey::ZygiskConfig) != 0,
            Ordering::Release,
        );
        info!("post_fs_data: zygisk_enabled={}", self.zygisk_enabled.load(Ordering::Relaxed));

        initialize_denylist();
        debug!("post_fs_data: handling modules");
        self.handle_modules();
        debug!("post_fs_data: handle_modules done");

        // 创建 zygisk.so symlink（ptrace 注入器 dlopen 需要此路径）
        let zygisk_link = cstr::buf::default()
            .join_path(get_magisk_tmp())
            .join_path(cstr!("zygisk.so"));
        if !zygisk_link.exists() {
            if unsafe { libc::symlink(
                    cstr!("/proc/self/exe").as_ptr(),
                    zygisk_link.as_ptr(),
                ) } != 0
            {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EEXIST) {
                    error!("failed to create zygisk.so symlink: {err}");
                }
            }
        }

        if self.zygisk_enabled.load(Ordering::Acquire) {
            info!("post_fs_data: starting zygisk init_monitor");
            crate::ffi::start_zygisk_monitor();
        }

        debug!("post_fs_data: clean_mounts");
        clean_mounts();

        diag_sdcard("post_fs_data");
        debug!("post_fs_data: done");

        false
    }

    fn late_start(&self) {
        setup_logfile();
        diag(2, "late_start");
        info!("** late_start service mode running");

        debug!("late_start: executing common scripts");
        exec_common_scripts(cstr!("service"));
        if let Some(module_list) = self.module_list.get() {
            info!("late_start: executing module scripts");
            exec_module_scripts(cstr!("service"), module_list);
        }
        
        diag_sdcard("late_start");
        debug!("late_start: done");
    }

    fn boot_complete(&self) {
        setup_logfile();
        diag(3, "boot_complete");
        info!("** boot-complete triggered");

        // Reset the bootloop counter once we have boot-complete
        debug!("boot_complete: resetting bootloop counter");
        self.set_db_setting(DbEntryKey::BootloopCount, 0).log_ok();

        // Mount MagiskSU (Kitsune Mask feature)
        // init 层（rootdir.cpp）已处理 recreate_sbin 后的 WORKERDIR 重建，
        // 此处为 daemon 层防御性兜底，防止极低概率的竞态。
        let worker_dir = cstr::buf::default()
            .join_path(get_magisk_tmp())
            .join_path(WORKERDIR);
        if !worker_dir.exists() {
            warn!("boot_complete: WORKERDIR ({worker_dir}) missing, creating");
            worker_dir.mkdir(0o755).log_ok();
        }
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
            self.zygisk.lock().reset(true);
        }

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

fn diag(tag: u32, stage: &str) {
    let ns = std::fs::read_link("/proc/self/ns/mnt").ok();
    let init_ns = std::fs::read_link("/proc/1/ns/mnt").ok();
    let sd_link = std::fs::read_link("/sdcard").ok();
    let sp_link = std::fs::read_link("/storage/self/primary").ok();
    let ms = std::fs::read_to_string("/proc/self/mountinfo")
        .ok().and_then(|s| s.lines()
            .find(|l| l.contains(" /storage "))
            .and_then(|l| l.split_whitespace()
                .find(|w| w.starts_with("shared:"))
                .map(|s| s.to_string())));
    debug!("D{} {} pid={} ns={:?} init_ns={:?} sd_a={} sd_lk={:?} sp_a={} sp_lk={:?} em_a={} da_a={} st={} sh={} ms={:?}",
        tag, stage, std::process::id(), ns, init_ns,
        cstr!("/sdcard").follow_link().exists(), sd_link,
        cstr!("/storage/self/primary").follow_link().exists(), sp_link,
        cstr!("/storage/emulated/0").follow_link().exists(),
        cstr!("/data/media/0").follow_link().exists(),
        cstr!("/storage").exists(),
        cstr!("/share").exists(), ms);
}

/// 诊断 sdcard 和 sdcardfs 挂载状态（在 post_fs_data 和 late_start 中复用）
fn diag_sdcard(stage: &str) {
    // 注意：使用 debug!() 而不是 info!()，因为 info!() 输出到 stdout，
    // 会被 boot_patch.sh 的 $(./magisk --preinit-device) 捕获，污染 PREINITDEVICE 变量
    debug!("{stage}: /sdcard exists={}", cstr!("/sdcard").exists());
    debug!("{stage}: /storage/self/primary exists={}", cstr!("/storage/self/primary").exists());
    debug!("{stage}: init.svc.vold={}", get_prop(cstr!("init.svc.vold")));

    let sdcardfs_mounts: Vec<_> = parse_mount_info("self")
        .into_iter()
        .filter(|info| info.fs_type == "sdcardfs")
        .collect();
    if sdcardfs_mounts.is_empty() {
        debug!("{stage}: sdcardfs NOT found in mountinfo");
    } else {
        for info in &sdcardfs_mounts {
            debug!("{stage}: sdcardfs mounted at {}", info.target);
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
