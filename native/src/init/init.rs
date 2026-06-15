// 标准库
use std::ffi::{CStr, c_char};
use std::ptr::null;

// 外部 crate
use base::libc::{basename, getpid, mount, umask};
use base::{LibcReturn, LoggedResult, ResultExt, cstr, debug, info, raw_cstr};

// 内部模块
use crate::ffi::{BootConfig, MagiskInit, backup_init, magisk_proxy_main};
use crate::logging::setup_klog;
use crate::mount::is_rootfs;
use crate::twostage::hexpatch_init_for_second_stage;

impl MagiskInit {
    fn new(argv: *mut *mut c_char) -> Self {
        Self {
            preinit_dev: String::new(),
            mount_list: Vec::new(),
            overlay_con: Vec::new(),
            argv,
            config: BootConfig {
                skip_initramfs: false,
                force_normal_boot: false,
                rootwait: false,
                emulator: false,
                slot: [0; 3],
                dt_dir: [0; 64],
                fstab_suffix: [0; 32],
                hardware: [0; 32],
                hardware_plat: [0; 32],
                boot_mode: [0; 16],
                partition_map: Vec::new(),
            },
        }
    }

    fn first_stage(&self) {
        info!("First Stage Init");
        
        // 添加系统环境探测（仅 debug 构建生效）
        #[cfg(debug_assertions)]
        crate::env_detect::detect_system_environment();
        
        self.prepare_data();

        let sdcard_exists = cstr!("/sdcard").exists();
        let first_stage_sdcard_exists = cstr!("/first_stage_ramdisk/sdcard").exists();
        info!("first_stage: /sdcard exists={}, /first_stage_ramdisk/sdcard exists={}", sdcard_exists, first_stage_sdcard_exists);

        if !sdcard_exists && !first_stage_sdcard_exists {
            // 先尝试 hexpatch（与 27.0 一致，适用于 VPhoneOS 等不支持 SwitchRoot 的环境）
            self.restore_ramdisk_init();
            info!("first_stage: calling hexpatch_init_for_second_stage(true)");
            let hexpatch_success = hexpatch_init_for_second_stage(true);
            info!("first_stage: hexpatch_init_for_second_stage result={}", hexpatch_success);
            
            // 如果 hexpatch 失败，fallback 到 hijack 方法
            if !hexpatch_success {
                info!("first_stage: hexpatch failed, fallback to hijack_init_with_switch_root");
                self.hijack_init_with_switch_root();
            }
        } else {
            self.restore_ramdisk_init();
            hexpatch_init_for_second_stage(true);
        }
    }

    fn second_stage(&mut self) {
        info!("Second Stage Init start");

        debug!("second_stage: unmounting /init");
        cstr!("/init").unmount().ok();
        debug!("second_stage: unmounting /system/bin/init");
        cstr!("/system/bin/init").unmount().ok();
        debug!("second_stage: removing /data/init");
        cstr!("/data/init").remove().ok();

        unsafe {
            debug!("second_stage: setting argv[0] = /system/bin/init");
            *self.argv = raw_cstr!("/system/bin/init") as *mut _;
        }

        let is_rootfs = is_rootfs();
        debug!("second_stage: is_rootfs={}", is_rootfs);

        if is_rootfs {
            info!("second_stage: still on rootfs, using patch_rw_root");
            let init_path = cstr!("/init");
            init_path.remove().ok();
            init_path
                .create_symlink_to(cstr!("/system/bin/init"))
                .log_ok();
            self.patch_rw_root();
        } else {
            info!("second_stage: using patch_ro_root");
            self.patch_ro_root();
        }

        info!("second_stage: done");
    }

    fn legacy_system_as_root(&mut self) {
        info!("Legacy SAR Init");
        self.prepare_data();
        let is_two_stage = self.mount_system_root();
        if is_two_stage {
            hexpatch_init_for_second_stage(false);
        } else {
            self.patch_ro_root();
        }
    }

    fn rootfs(&mut self) {
        info!("RootFS Init");
        self.prepare_data();
        self.restore_ramdisk_init();
        self.patch_rw_root();
    }

    fn recovery_or_charger(&self) {
        info!("Charger mode or ramdisk is recovery, abort");
        self.restore_ramdisk_init();
        cstr!("/.backup").remove_all().ok();
    }

    fn restore_ramdisk_init(&self) {
        cstr!("/init").remove().ok();

        let orig_init = backup_init();

        if orig_init.exists() {
            orig_init.rename_to(cstr!("/init")).log_ok();
        } else {
            // If the backup init is missing, this means that the boot ramdisk
            // was created from scratch, and the real init is in a separate CPIO,
            // which is guaranteed to be placed at /system/bin/init.
            cstr!("/init")
                .create_symlink_to(cstr!("/system/bin/init"))
                .log_ok();
        }
    }

    fn start(&mut self) -> LoggedResult<()> {
        info!("MagiskInit::start() begin");
        
        // 挂载 /proc
        if !cstr!("/proc/cmdline").exists() {
            debug!("start: mounting /proc");
            cstr!("/proc").mkdir(0o755)?;
            unsafe {
                mount(
                    raw_cstr!("proc"),
                    raw_cstr!("/proc"),
                    raw_cstr!("proc"),
                    0,
                    null(),
                )
            }
            .check_err()?;
            self.mount_list.push("/proc".to_string());
            debug!("start: /proc mounted");
        }
        
        // 挂载 /sys
        if !cstr!("/sys/block").exists() {
            debug!("start: mounting /sys");
            cstr!("/sys").mkdir(0o755)?;
            unsafe {
                mount(
                    raw_cstr!("sysfs"),
                    raw_cstr!("/sys"),
                    raw_cstr!("sysfs"),
                    0,
                    null(),
                )
            }
            .check_err()?;
            self.mount_list.push("/sys".to_string());
            debug!("start: /sys mounted");
        }
        
        debug!("start: calling setup_klog()");
        setup_klog();
        
        debug!("start: calling config.init()");
        self.config.init();
        debug!("start: config.init() done");
        
        let argv1 = unsafe { *self.argv.offset(1) };
        let is_selinux_setup = !argv1.is_null() && unsafe { CStr::from_ptr(argv1) == c"selinux_setup" };
        debug!("start: is_selinux_setup={}", is_selinux_setup);
        
        if is_selinux_setup {
            info!("start: calling second_stage()");
            self.second_stage();
        } else if self.config.skip_initramfs {
            info!("start: calling legacy_system_as_root()");
            self.legacy_system_as_root();
        } else if self.config.force_normal_boot {
            info!("start: calling first_stage() [force_normal_boot=true]");
            self.first_stage();
        } else if cstr!("/sbin/recovery").exists()
            || cstr!("/system/bin/recovery").exists()
            || unsafe { CStr::from_ptr(self.config.boot_mode.as_ptr()) } == c"charger"
        {
            info!("start: calling recovery_or_charger()");
            self.recovery_or_charger();
        } else if self.check_two_stage() {
            info!("start: calling first_stage() [check_two_stage=true]");
            self.first_stage();
        } else {
            info!("start: calling rootfs()");
            self.rootfs();
        }
        
        info!("start: calling exec_init()");
        self.exec_init();
        
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(
    argc: i32,
    argv: *mut *mut c_char,
    _envp: *const *const c_char,
) -> i32 {
    unsafe {
        umask(0);

        let name = basename(*argv);

        if CStr::from_ptr(name) == c"magisk" {
            return magisk_proxy_main(argc, argv);
        }

        if getpid() == 1 {
            MagiskInit::new(argv).start().log_ok();
        }

        1
    }
}
