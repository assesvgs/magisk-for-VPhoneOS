use core::ffi::c_void;
use crate::jni_env::JNINativeMethod;

// ===== ZygiskModuleApi（模块入口收到的结构体） =====
#[repr(C)]
pub struct ZygiskModuleApi {
    pub base: *mut c_void,       // → ZygiskModuleImpl
    pub impl_size: u32,
    pub module: *mut c_void,
}

// ===== ZygiskModuleImpl（框架→模块的生命周期回调） =====
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ZygiskModuleImpl {
    pub pre_app_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub post_app_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub pre_server_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub post_server_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
}

// ===== ModuleApiV1（模块→框架的 API，模块调框架） =====
#[repr(C)]
pub struct ModuleApiV1 {
    pub handle: *mut c_void,
    pub hook_jni_native_methods: Option<
        unsafe extern "C" fn(*mut c_void, *const libc::c_char, *mut JNINativeMethod, i32) -> bool
    >,
    pub plt_hook_register: Option<
        unsafe extern "C" fn(u64, u64, *const libc::c_char, *mut c_void, *mut *mut c_void) -> bool
    >,
    pub plt_hook_exclude: Option<
        unsafe extern "C" fn(u64, u64, *const libc::c_char) -> bool
    >,
    pub plt_hook_commit: Option<unsafe extern "C" fn() -> bool>,
    pub connect_companion: Option<unsafe extern "C" fn(i32)>,
    pub set_option: Option<unsafe extern "C" fn(u32)>,
}

#[repr(C)]
pub struct ModuleApiV2 {
    pub v1: ModuleApiV1,
    pub get_module_dir: Option<unsafe extern "C" fn(i32) -> i32>,
    pub get_flags: Option<unsafe extern "C" fn() -> u32>,
}

#[repr(C)]
pub struct ModuleApiV4 {
    pub v2: ModuleApiV2,
    pub plt_hook_register_v4: Option<
        unsafe extern "C" fn(u64, u64, *const libc::c_char, *mut c_void, *mut *mut c_void) -> bool
    >,
    pub plt_hook_commit_v4: Option<unsafe extern "C" fn() -> bool>,
    pub exempt_fd: Option<unsafe extern "C" fn(i32) -> bool>,
}

// ===== Default implementations =====

unsafe extern "C" fn default_plt_register(
    _dev: u64, _ino: u64, sym: *const libc::c_char, hook: *mut c_void, orig: *mut *mut c_void,
) -> bool {
    if sym.is_null() { return false; }
    let sym_bytes = core::ffi::CStr::from_ptr(sym).to_bytes_with_nul();
    let maps = crate::plt::scan_maps();
    if maps.is_empty() { return false; }
    // 在所有加载的库中搜索此符号
    for map in &maps {
        if crate::plt::find_and_hook(&maps, &map.path, sym_bytes, hook, orig) {
            return true;
        }
    }
    false
}

unsafe extern "C" fn default_plt_commit() -> bool {
    true
}

unsafe extern "C" fn default_hook_jni(
    env: *mut c_void, clz: *const libc::c_char, methods: *mut JNINativeMethod, n: i32,
) -> bool {
    let functions = unsafe { *(env as *mut *mut c_void) } as *mut *mut c_void;
    if functions.is_null() { return false; }

    let find_class_addr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"JNI_FindClass\0".as_ptr() as *const libc::c_char) };
    if find_class_addr.is_null() { return false; }

    let max_entries = 256;
    let mut fc_offset: Option<usize> = None;
    for i in 0..max_entries {
        if unsafe { *(functions as *mut *mut c_void).add(i) } == find_class_addr {
            fc_offset = Some(i);
            break;
        }
    }
    let fc_offset = match fc_offset { Some(o) => o, None => return false };

    type FindClassFn = unsafe extern "C" fn(*mut c_void, *const libc::c_char) -> *mut c_void;
    let find_class: FindClassFn = unsafe { core::mem::transmute(functions.add(fc_offset)) };

    let clazz = unsafe { find_class(env, clz) };
    if clazz.is_null() { return false; }

    let orig_reg = crate::jni_env::get_orig_register_natives();
    if let Some(f) = orig_reg {
        let ret = f(env, clazz, methods, n);
        ret >= 0
    } else { false }
}

unsafe extern "C" fn default_connect_companion(_fd: i32) {
    crate::ipc::connect_companion(_fd);
}

unsafe extern "C" fn default_set_option(_option: u32) {}

unsafe extern "C" fn default_get_module_dir(_id: i32) -> i32 { -1 }

unsafe extern "C" fn default_get_flags() -> u32 { 0 }

// ===== Vtable 填充 =====
impl ModuleApiV1 {
    pub fn populate(table: &mut ModuleApiV1, handle: *mut c_void) {
        table.handle = handle;
        table.hook_jni_native_methods = Some(default_hook_jni);
        table.plt_hook_register = Some(default_plt_register);
        table.plt_hook_exclude = None;
        table.plt_hook_commit = Some(default_plt_commit);
        table.connect_companion = Some(default_connect_companion);
        table.set_option = Some(default_set_option);
    }
}

impl ModuleApiV2 {
    pub fn populate(table: &mut ModuleApiV2, handle: *mut c_void) {
        ModuleApiV1::populate(&mut table.v1, handle);
        table.get_module_dir = Some(default_get_module_dir);
        table.get_flags = Some(default_get_flags);
    }
}

impl ModuleApiV4 {
    pub fn populate(table: &mut ModuleApiV4, handle: *mut c_void) {
        ModuleApiV2::populate(&mut table.v2, handle);
        table.plt_hook_register_v4 = Some(default_plt_register);
        table.plt_hook_commit_v4 = Some(default_plt_commit);
        table.exempt_fd = None;
    }
}

// ===== 生命周期 dispatch =====
pub fn call_pre_app_specialize(api_handle: *mut c_void) {
    if api_handle.is_null() { return; }
    let api = api_handle as *mut ZygiskModuleApi;
    let impl_ptr = unsafe { (*api).base as *mut ZygiskModuleImpl };
    if impl_ptr.is_null() { return; }
    if let Some(handler) = unsafe { (*impl_ptr).pre_app_specialize } {
        unsafe { handler(api_handle) };
    }
}

pub fn call_post_app_specialize(api_handle: *mut c_void) {
    if api_handle.is_null() { return; }
    let api = api_handle as *mut ZygiskModuleApi;
    let impl_ptr = unsafe { (*api).base as *mut ZygiskModuleImpl };
    if impl_ptr.is_null() { return; }
    if let Some(handler) = unsafe { (*impl_ptr).post_app_specialize } {
        unsafe { handler(api_handle) };
    }
}

// ===== SpinMutex (no_std 自旋锁) =====
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for SpinMutex<T> {}

impl<T> SpinMutex<T> {
    pub const fn new(data: T) -> Self {
        Self { locked: AtomicBool::new(false), data: UnsafeCell::new(data) }
    }
    pub fn lock(&self) -> SpinMutexGuard<T> {
        while self.locked.swap(true, Ordering::Acquire) { core::hint::spin_loop(); }
        SpinMutexGuard { mutex: self }
    }
}

pub struct SpinMutexGuard<'a, T> {
    mutex: &'a SpinMutex<T>,
}

impl<'a, T> core::ops::Deref for SpinMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T { unsafe { &*self.mutex.data.get() } }
}

impl<'a, T> core::ops::DerefMut for SpinMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T { unsafe { &mut *self.mutex.data.get() } }
}

impl<'a, T> Drop for SpinMutexGuard<'a, T> {
    fn drop(&mut self) { self.mutex.locked.store(false, Ordering::Release); }
}

// ===== 全局 PLT hook 列表（供 unhook 使用） =====
pub struct PltHookEntry {
    pub dev: u64,
    pub ino: u64,
    pub addr: usize,
    pub perms: i32,
    pub sym: alloc::vec::Vec<u8>,
    pub orig: *mut c_void,
}

static PLT_HOOK_LIST: SpinMutex<alloc::vec::Vec<PltHookEntry>> = SpinMutex::new(alloc::vec::Vec::new());

pub fn push_plt_hook(dev: u64, ino: u64, addr: usize, perms: i32, sym: &[u8], orig: *mut c_void) {
    PLT_HOOK_LIST.lock().push(PltHookEntry { dev, ino, addr, perms, sym: sym.to_vec(), orig });
}

pub fn acquire_for_fork() -> SpinMutexGuard<'static, alloc::vec::Vec<PltHookEntry>> {
    // fork 前获取锁，fork 后释放，确保子进程看到 unlocked 状态
    PLT_HOOK_LIST.lock()
}

pub fn release_after_fork() {
    // fork 后强制解锁——子进程中锁状态已拷贝，需手动释放
    unsafe { PLT_HOOK_LIST.locked.store(false, Ordering::Release); }
}

pub fn get_plt_hook_list() -> SpinMutexGuard<'static, alloc::vec::Vec<PltHookEntry>> {
    PLT_HOOK_LIST.lock()
}

// ===== Default trait impls =====
impl Default for ModuleApiV1 {
    fn default() -> Self {
        Self {
            handle: core::ptr::null_mut(),
            hook_jni_native_methods: None,
            plt_hook_register: None,
            plt_hook_exclude: None,
            plt_hook_commit: None,
            connect_companion: None,
            set_option: None,
        }
    }
}

impl Default for ModuleApiV2 {
    fn default() -> Self {
        Self {
            v1: Default::default(),
            get_module_dir: None,
            get_flags: None,
        }
    }
}

impl Default for ModuleApiV4 {
    fn default() -> Self {
        Self {
            v2: Default::default(),
            plt_hook_register_v4: None,
            plt_hook_commit_v4: None,
            exempt_fd: None,
        }
    }
}
