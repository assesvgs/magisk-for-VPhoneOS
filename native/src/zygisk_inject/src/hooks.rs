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
    unsafe { f(flags) }
}

extern "C" fn new_selinux_android_setcontext(
    uid: u32, is_system_server: i32,
    seinfo: *const libc::c_char, pkgname: *const libc::c_char,
) -> i32 {
    let f: SetcontextFn = match orig_fn(HookSlot::Setcontext) { Some(f) => f, None => return -1 };
    unsafe { f(uid, is_system_server, seinfo, pkgname) }
}

extern "C" fn new_strdup(s: *const libc::c_char) -> *mut libc::c_char {
    let f: StrdupFn = match orig_fn(HookSlot::Strdup) { Some(f) => f, None => return core::ptr::null_mut() };
    unsafe { f(s) }
}

extern "C" fn new_android_log_close() {
    let f: LogCloseFn = match orig_fn(HookSlot::LogClose) { Some(f) => f, None => return };
    unsafe { f() }
}

// 拦截 libnativebridge.so 的 dlclose（Magisk 惯用做法，确保模块在 zygote 生命周期内常驻）
extern "C" fn new_dlclose(handle: *mut c_void) -> i32 {
    let f: DlcloseFn = match orig_fn(HookSlot::Dlclose) { Some(f) => f, None => return -1 };
    unsafe { f(handle) }
}

extern "C" fn new_android_set_create_thread(_func: *mut c_void) {
    // pass-through, no JNI hook for now
}

extern "C" fn new_pthread_attr_destroy(attr: *mut c_void) -> i32 {
    let f: PthreadAttrDestroyFn = match orig_fn(HookSlot::PthreadAttrDestroy) {
        Some(f) => f, None => return -1
    };
    unsafe { f(attr) }
}

pub fn install_hooks(_handle: *mut c_void) {
    // 完全无操作——不保存 handle，不 hook_plt
    // 测试：仅加载库 + 入口函数返回后 zygote 是否稳定
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
    ("/libnativebridge.so", b"dlclose\0",                new_dlclose as *mut c_void, HookSlot::Dlclose),
    ("/libandroid_runtime.so", b"unshare\0",              new_unshare as *mut c_void, HookSlot::Unshare),
    ("/libandroid_runtime.so", b"selinux_android_setcontext\0",
     new_selinux_android_setcontext as *mut c_void, HookSlot::Setcontext),
    ("/libandroid_runtime.so", b"strdup\0",               new_strdup as *mut c_void, HookSlot::Strdup),
    ("/libandroid_runtime.so", b"__android_log_close\0",  new_android_log_close as *mut c_void, HookSlot::LogClose),
    ("/libandroid_runtime.so", b"androidSetCreateThread\0",
     new_android_set_create_thread as *mut c_void, HookSlot::AndroidSetCreateThread),
    ("/libc.so", b"pthread_attr_destroy\0",
     new_pthread_attr_destroy as *mut c_void, HookSlot::PthreadAttrDestroy),
];
