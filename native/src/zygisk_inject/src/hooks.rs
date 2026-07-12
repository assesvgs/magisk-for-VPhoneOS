use core::ffi::c_void;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

type UnshareFn = unsafe extern "C" fn(i32) -> i32;
type SetcontextFn = unsafe extern "C" fn(u32, i32, *const libc::c_char, *const libc::c_char) -> i32;
type StrdupFn = unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_char;
type LogCloseFn = unsafe extern "C" fn();
type DlcloseFn = unsafe extern "C" fn(*mut c_void) -> i32;
type PthreadAttrDestroyFn = unsafe extern "C" fn(*mut c_void) -> i32;

#[derive(Copy, Clone)]
enum HookSlot {
    Unshare,
    Setcontext,
    Strdup,
    LogClose,
    Dlclose,
    AndroidSetCreateThread,
    PthreadAttrDestroy,
}

struct OrigFuncs([UnsafeCell<*mut c_void>; 7]);
// 只在单线程初始化期被 C++ FFI 写入，之后只读。Sync 安全。
unsafe impl Sync for OrigFuncs {}
const ORIG_FUNC_INIT: UnsafeCell<*mut c_void> = UnsafeCell::new(core::ptr::null_mut());

static ORIG_FUNCS: OrigFuncs = OrigFuncs([
    ORIG_FUNC_INIT, ORIG_FUNC_INIT, ORIG_FUNC_INIT, ORIG_FUNC_INIT,
    ORIG_FUNC_INIT, ORIG_FUNC_INIT, ORIG_FUNC_INIT,
]);
// Zygote 初始化阶段是单线程的，Relaxed 安全
static ZYGOTE_INIT_SEEN: AtomicBool = AtomicBool::new(false);

fn orig_ptr(slot: HookSlot) -> *mut *mut c_void {
    ORIG_FUNCS.0[slot as usize].get()
}

fn orig_fn<T>(slot: HookSlot) -> Option<T> {
    let p = unsafe { *ORIG_FUNCS.0[slot as usize].get() };
    if p.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

extern "C" fn new_unshare(flags: i32) -> i32 {
    let f: UnshareFn = match orig_fn(HookSlot::Unshare) { Some(f) => f, None => return -1 };
    let res = unsafe { f(flags) };
    if res == 0 && (flags & libc::CLONE_NEWNS) != 0 {
        if let Some(ctx) = crate::hook_context::current_ctx() {
            if ctx.flags.has(crate::hook_context::Flags::DO_ALLOW) {
                crate::ipc::request_sulist();
            } else if !ctx.flags.has(crate::hook_context::Flags::ALLOWLIST_ENFORCED)
                && ctx.flags.has(crate::hook_context::Flags::DO_REVERT_UNMOUNT)
            {
                crate::ipc::request_umount();
            }
            // 二次 unshare 创建空白挂载 ID 空洞（Magisk 惯例），不是意外调用
            if unsafe { f(libc::CLONE_NEWNS) } != 0 {
                // 二次 unshare 失败—挂载 ID 空洞未修复，不影响主要功能
            }
            if ctx.flags.has(crate::hook_context::Flags::RESTORE_MOUNT_EXTERNAL_NONE) {
                // TODO: 通过 AppSpecializeArgs.mount_external 恢复
                // let args_ptr = ctx.args as *mut crate::proxy_gen::AppSpecializeArgs;
                // unsafe { *((*args_ptr).mount_external as *mut i32) = 0; }
            }
        }
    }
    res
}

extern "C" fn new_selinux_android_setcontext(
    uid: u32, is_system_server: i32,
    seinfo: *const libc::c_char, pkgname: *const libc::c_char,
) -> i32 {
    let f: SetcontextFn = match orig_fn(HookSlot::Setcontext) { Some(f) => f, None => return -1 };
    unsafe {
        libc::access(b"/dev/socket/logdw\0".as_ptr() as *const libc::c_char, libc::W_OK);
    }
    unsafe { f(uid, is_system_server, seinfo, pkgname) }
}

extern "C" fn new_strdup(s: *const libc::c_char) -> *mut libc::c_char {
    let f: StrdupFn = match orig_fn(HookSlot::Strdup) { Some(f) => f, None => return core::ptr::null_mut() };
    let ret = unsafe { f(s) };

    if !s.is_null() && !ZYGOTE_INIT_SEEN.load(Ordering::Relaxed) {
        let s_slice = unsafe { core::ffi::CStr::from_ptr(s) };
        if let Ok(s_str) = s_slice.to_str() {
            if s_str == "ZygoteInit" {
                ZYGOTE_INIT_SEEN.store(true, Ordering::Relaxed);
                crate::jni_env::hook_jni_env();
            }
        }
    }
    ret
}

extern "C" fn new_android_log_close() {
    let skip = crate::hook_context::current_ctx()
        .map(|ctx| ctx.flags.has(crate::hook_context::Flags::SKIP_CLOSE_LOG_PIPE))
        .unwrap_or(false);
    if skip { return; }
    let f: LogCloseFn = match orig_fn(HookSlot::LogClose) { Some(f) => f, None => return };
    unsafe { f() }
}

// 拦截 libnativebridge.so 的 dlclose（Magisk 惯用做法，确保模块在 zygote 生命周期内常驻）
extern "C" fn new_dlclose(handle: *mut c_void) -> i32 {
    let f: DlcloseFn = match orig_fn(HookSlot::Dlclose) { Some(f) => f, None => return -1 };
    unsafe { f(handle) }
}

extern "C" fn new_android_set_create_thread(_func: *mut c_void) {
    crate::jni_env::hook_jni_env();
}

extern "C" fn new_pthread_attr_destroy(attr: *mut c_void) -> i32 {
    let f: PthreadAttrDestroyFn = match orig_fn(HookSlot::PthreadAttrDestroy) {
        Some(f) => f, None => return -1
    };
    let ret = unsafe { f(attr) };
    if crate::unload::SHOULD_UNLOAD.load(Ordering::Acquire) {
        crate::unload::unhook_functions();
        unsafe { crate::unload::dlclose_self(
            crate::unload::SELF_HANDLE.load(Ordering::Relaxed)
        ) }
    }
    ret
}

pub fn install_hooks(handle: *mut c_void) {
    crate::unload::save_self_handle(handle);
    hook_plt();
}

pub fn hook_plt() {
    let maps = crate::plt::scan_maps();
    if maps.is_empty() {
        return;
    }
    for &(lib, sym, hook, slot) in HOOK_LIST.iter() {
        crate::plt::find_and_hook(&maps, lib, sym, hook, orig_ptr(slot));
    }
    crate::plt::commit_all();
}

const HOOK_LIST: &[(&str, &[u8], *mut c_void, HookSlot)] = &[
    ("/libnativebridge.so", b"dlclose\0", new_dlclose as *mut c_void, HookSlot::Dlclose),
    ("/libandroid_runtime.so", b"unshare\0",                new_unshare as *mut c_void, HookSlot::Unshare),
    ("/libandroid_runtime.so", b"selinux_android_setcontext\0",
     new_selinux_android_setcontext as *mut c_void, HookSlot::Setcontext),
    ("/libandroid_runtime.so", b"strdup\0",                 new_strdup as *mut c_void, HookSlot::Strdup),
    ("/libandroid_runtime.so", b"__android_log_close\0",    new_android_log_close as *mut c_void, HookSlot::LogClose),
    ("/libandroid_runtime.so", b"androidSetCreateThread\0",
     new_android_set_create_thread as *mut c_void, HookSlot::AndroidSetCreateThread),
    ("/libc.so", b"pthread_attr_destroy\0",
     new_pthread_attr_destroy as *mut c_void, HookSlot::PthreadAttrDestroy),
];
