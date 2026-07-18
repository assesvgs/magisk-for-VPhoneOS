use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, Ordering};

// ===== Flags =====
pub struct Flags(u16);
impl Flags {
    pub const POST_SPECIALIZE: u16            = 1 << 0;
    pub const APP_FORK_AND_SPECIALIZE: u16    = 1 << 1;
    pub const APP_SPECIALIZE: u16             = 1 << 2;
    pub const SERVER_FORK_AND_SPECIALIZE: u16 = 1 << 3;
    pub const DO_REVERT_UNMOUNT: u16          = 1 << 4;
    pub const SKIP_CLOSE_LOG_PIPE: u16        = 1 << 5;
    pub const DO_ALLOW: u16                   = 1 << 6;
    pub const ALLOWLIST_ENFORCED: u16         = 1 << 7;
    pub const RESTORE_MOUNT_EXTERNAL_NONE: u16 = 1 << 8;
    pub const DO_FUTILE_HIDE: u16             = 1 << 9;
    pub const DO_ALLOW_SU: u16                = 1 << 10;

    pub fn new() -> Self { Flags(0) }
    pub fn set(&mut self, flag: u16) { self.0 |= flag; }
    pub fn has(&self, flag: u16) -> bool { (self.0 & flag) != 0 }
    pub fn clear(&mut self, flag: u16) { self.0 &= !flag; }
    pub fn raw(&self) -> u16 { self.0 }
}

// ===== HookContext =====
pub struct HookContext {
    pub env: *mut c_void,
    pub args: *mut c_void,
    pub process_name: alloc::string::String,
    pub pid: i32,
    pub flags: Flags,
    pub info_flags: u32,
    pub modules: alloc::vec::Vec<crate::module::ZygiskModule>,
    pub allowed_fds: crate::fd::FdSet,
    pub exempted_fds: alloc::vec::Vec<i32>,
}

static CURRENT_CTX: AtomicPtr<HookContext> = AtomicPtr::new(core::ptr::null_mut());

pub fn current_ctx() -> Option<&'static mut HookContext> {
    let ptr = CURRENT_CTX.load(Ordering::Acquire);
    if ptr.is_null() { None } else { Some(unsafe { &mut *ptr }) }
}

pub fn set_current(ctx: &mut HookContext) {
    CURRENT_CTX.store(ctx as *mut HookContext, Ordering::Release);
}

pub fn unset_current() {
    CURRENT_CTX.store(core::ptr::null_mut(), Ordering::Release);
}

pub fn get_current_ptr() -> *mut HookContext {
    CURRENT_CTX.load(Ordering::Acquire)
}

pub fn set_current_ptr(ptr: *mut HookContext) {
    if ptr.is_null() {
        CURRENT_CTX.store(core::ptr::null_mut(), Ordering::Release);
    } else {
        CURRENT_CTX.store(ptr, Ordering::Release);
    }
}

impl HookContext {
    pub fn new(
        env: *mut c_void,
        args: *mut c_void,
        process_name: &str,
    ) -> Self {
        HookContext {
            env,
            args,
            process_name: alloc::string::String::from(process_name),
            pid: -1,
            flags: Flags::new(),
            info_flags: 0,
            modules: alloc::vec::Vec::new(),
            allowed_fds: crate::fd::FdSet::new(),
            exempted_fds: alloc::vec::Vec::new(),
        }
    }

    pub fn fork_pre(&mut self) {
        let mut set: libc::sigset_t = unsafe { core::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGCHLD);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, core::ptr::null_mut());
        }
        crate::fd::record_open_fds(&mut self.allowed_fds);
        for fd in 0..3 { self.allowed_fds.add(fd); }
        core::mem::forget(crate::module_api::acquire_for_fork());
    }

    pub fn fork_post(&mut self) {
        let mut set: libc::sigset_t = unsafe { core::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut set);
            libc::pthread_sigmask(libc::SIG_SETMASK, &set, core::ptr::null_mut());
        }
        crate::module_api::release_after_fork();
    }

    pub fn app_specialize_pre(&mut self) {
        let uid = if !self.args.is_null() {
            unsafe {
                let args = self.args as *const crate::proxy_gen::AppSpecializeArgs;
                let uid_ptr = (*args).uid as *const i32;
                // 若 mount_external == 0 (MOUNT_EXTERNAL_NONE)，临时提升为 MOUNT_EXTERNAL_DEFAULT，
                // 以确保创建独立挂载命名空间（unshare CLONE_NEWNS），并记下 flag 以便在 new_unshare hook 中恢复。
                let mount_ext_ptr = (*args).mount_external as *const i32;
                if !mount_ext_ptr.is_null() && *mount_ext_ptr == 0 {
                    *(mount_ext_ptr as *mut i32) = 1;  // → MOUNT_EXTERNAL_DEFAULT
                    self.flags.set(Flags::RESTORE_MOUNT_EXTERNAL_NONE);
                }
                *uid_ptr
            }
        } else { 0 };
        if let Some((info_flags, fds)) = crate::ipc::remote_get_info(uid, &self.process_name) {
            self.info_flags = info_flags;
            if (info_flags & 1) != 0 { self.flags.set(Flags::DO_REVERT_UNMOUNT); }
            if (info_flags & 2) != 0 { self.flags.set(Flags::DO_ALLOW); }
            if (info_flags & 4) != 0 { self.flags.set(Flags::ALLOWLIST_ENFORCED); }
            if (info_flags & 8) != 0 { self.flags.set(Flags::DO_FUTILE_HIDE); }
            self.run_modules_pre(&fds);
        }
    }

    pub fn app_specialize_post(&mut self) {
        self.sanitize_fds();
        self.fork_post();
    }

    pub fn server_specialize_pre(&mut self) {
        let uid = 1000;
        if let Some((info_flags, fds)) = crate::ipc::remote_get_info(uid, &self.process_name) {
            self.info_flags = info_flags;
            self.run_modules_pre_server(&fds);
        }
    }

    pub fn server_specialize_post(&mut self) {
        // system_server fork 子进程：模块在父进程中未加载（只获取了 info_flags），
        // 需在此加载模块并派发 pre + post 回调。
        if self.modules.is_empty() {
            if let Some((_info_flags, fds)) = crate::ipc::remote_get_info(1000, &self.process_name) {
                self.run_modules_pre_server(&fds);
            }
        }
        self.run_modules_post_server();
        crate::solist::hide_modules();
        self.sanitize_fds();
        self.fork_post();
    }

    pub fn native_fork_and_specialize_pre(&mut self) {
        self.fork_pre();
        // 在 fork 前从 daemon 获取进程信息（info_flags），
        // 使得 fork 后子进程的 new_unshare 等 hook 能访问正确的 flag。
        // 注意：此处不加载模块——fork 后子进程自行加载。
        let uid = if !self.args.is_null() {
            unsafe {
                let args = self.args as *const crate::proxy_gen::AppSpecializeArgs;
                // 若 mount_external == 0 (MOUNT_EXTERNAL_NONE)，临时提升为 1 (DEFAULT)，
                // 确保 unshare(CLONE_NEWNS) 创建独立挂载命名空间；
                // new_unshare hook 会看到 RESTORE_MOUNT_EXTERNAL_NONE flag 并在二次 unshare 后恢复。
                let mount_ext_ptr = (*args).mount_external as *const i32;
                if !mount_ext_ptr.is_null() && *mount_ext_ptr == 0 {
                    *(mount_ext_ptr as *mut i32) = 1;
                    self.flags.set(Flags::RESTORE_MOUNT_EXTERNAL_NONE);
                }
                let uid_ptr = (*args).uid as *const i32;
                *uid_ptr
            }
        } else { 0 };
        if let Some((info_flags, _fds)) = crate::ipc::remote_get_info(uid, &self.process_name) {
            self.info_flags = info_flags;
            if (info_flags & 1) != 0 { self.flags.set(Flags::DO_REVERT_UNMOUNT); }
            if (info_flags & 2) != 0 { self.flags.set(Flags::DO_ALLOW); }
            if (info_flags & 4) != 0 { self.flags.set(Flags::ALLOWLIST_ENFORCED); }
            if (info_flags & 8) != 0 { self.flags.set(Flags::DO_FUTILE_HIDE); }
        }
    }

    pub fn native_fork_and_specialize_post(&mut self) {
        self.fork_post();
        self.sanitize_fds();
    }

    pub fn native_specialize_app_process_pre(&mut self) {
        let uid = if !self.args.is_null() {
            unsafe {
                let args = self.args as *const crate::proxy_gen::AppSpecializeArgs;
                let uid_ptr = (*args).uid as *const i32;
                *uid_ptr
            }
        } else { 0 };
        if let Some((info_flags, fds)) = crate::ipc::remote_get_info(uid, &self.process_name) {
            self.info_flags = info_flags;
            if (info_flags & 16) != 0 { self.flags.set(Flags::DO_ALLOW_SU); }
            self.run_modules_pre(&fds);
        }
    }

    pub fn native_specialize_app_process_post(&mut self) {
        // 从 fork+specialize 子进程路径调用时，模块尚未在子进程中加载。
        // 需先加载模块并派发 pre 回调，再派发 post 回调。
        if self.modules.is_empty() {
            let uid = if !self.args.is_null() {
                unsafe {
                    let args = self.args as *const crate::proxy_gen::AppSpecializeArgs;
                    let uid_ptr = (*args).uid as *const i32;
                    *uid_ptr
                }
            } else { 0 };
            if let Some((_info_flags, fds)) = crate::ipc::remote_get_info(uid, &self.process_name) {
                self.run_modules_pre(&fds);
            }
        }
        self.run_modules_post();
        if self.flags.has(Flags::DO_FUTILE_HIDE) {
            crate::solist::hide_modules();
        }
        if self.info_flags & 16 != 0 {
            unsafe {
                libc::setenv(b"ZYGISK_ENABLED\0".as_ptr() as *const libc::c_char,
                             b"1\0".as_ptr() as *const libc::c_char, 1);
            }
        }
        self.sanitize_fds();
    }

    pub fn native_fork_system_server_pre(&mut self) {
        self.fork_pre();
        // 父进程：获取 info_flags（子进程通过 COW 继承），
        // 但不在此加载或派发模块回调——fork 后子进程自行完成。
        if let Some((info_flags, _fds)) = crate::ipc::remote_get_info(1000, &self.process_name) {
            self.info_flags = info_flags;
        }
    }

    pub fn native_fork_system_server_post(&mut self) {
        // 子进程（system_server）：加载模块（如有必要），
        // 派发 pre 回调，再派发 post 回调。
        if self.modules.is_empty() {
            if let Some((_info_flags, fds)) = crate::ipc::remote_get_info(1000, &self.process_name) {
                self.run_modules_pre_server(&fds);
            }
        }
        self.run_modules_post_server();
        self.sanitize_fds();
        self.fork_post();
    }

    pub fn run_modules_pre(&mut self, fds: &[i32]) { self.run_modules_pre_impl(fds); }
    pub fn run_modules_post(&mut self) { self.run_modules_post_impl(); }
    pub fn run_modules_pre_server(&mut self, fds: &[i32]) { self.run_modules_pre_server_impl(fds); }
    pub fn run_modules_post_server(&mut self) { self.run_modules_post_server_impl(); }
    pub fn sanitize_fds(&mut self) {
        crate::fd::sanitize_fds(&self.allowed_fds, &self.exempted_fds);
    }
    pub fn exempt_fd(&mut self, fd: i32) { self.exempted_fds.push(fd); }
}

impl Drop for HookContext {
    fn drop(&mut self) {
        let ptr = CURRENT_CTX.load(Ordering::Acquire);
        if ptr == self as *mut HookContext {
            CURRENT_CTX.store(core::ptr::null_mut(), Ordering::Release);
        }
        if !self.env.is_null() {
            crate::jni_env::restore_jni_env(self.env);
        }
        // Drop 中不设 SHOULD_UNLOAD——卸载应由子进程逻辑触发
    }
}
