use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

type ForkFn = unsafe extern "C" fn() -> i32;
type UnshareFn = unsafe extern "C" fn(i32) -> i32;
type SetcontextFn = unsafe extern "C" fn(u32, i32, *const libc::c_char, *const libc::c_char) -> i32;
type StrdupFn = unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_char;
type LogCloseFn = unsafe extern "C" fn();
type DlcloseFn = unsafe extern "C" fn(*mut c_void) -> i32;

#[derive(Copy, Clone)]
enum HookSlot {
    Fork,
    Unshare,
    Setcontext,
    Strdup,
    LogClose,
    Dlclose,
}

static mut ORIG_FUNCS: [*mut c_void; 6] = [core::ptr::null_mut(); 6];
static ZYGOTE_INIT_SEEN: AtomicBool = AtomicBool::new(false);

fn orig_ptr(slot: HookSlot) -> *mut *mut c_void {
    unsafe { &mut ORIG_FUNCS[slot as usize] as *mut *mut c_void }
}

fn orig_fn<T>(slot: HookSlot) -> Option<T> {
    let p = unsafe { ORIG_FUNCS[slot as usize] };
    if p.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

extern "C" fn new_fork() -> i32 {
    let f: ForkFn = match orig_fn(HookSlot::Fork) { Some(f) => f, None => return -1 };
    unsafe { f() }
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
    let ret = unsafe { f(s) };

    if !s.is_null() && !ZYGOTE_INIT_SEEN.load(Ordering::Relaxed) {
        let s_slice = unsafe { core::ffi::CStr::from_ptr(s) };
        if let Ok(s_str) = s_slice.to_str() {
            if s_str == "ZygoteInit" {
                ZYGOTE_INIT_SEEN.store(true, Ordering::Relaxed);
                crate::jni::hook_jni_env();
            }
        }
    }
    ret
}

extern "C" fn new_android_log_close() {
    let f: LogCloseFn = match orig_fn(HookSlot::LogClose) { Some(f) => f, None => return };
    unsafe { f() }
}

extern "C" fn new_dlclose(handle: *mut c_void) -> i32 {
    let f: DlcloseFn = match orig_fn(HookSlot::Dlclose) { Some(f) => f, None => return -1 };
    unsafe { f(handle) }
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
    ("/libandroid_runtime.so", b"fork\0",                    new_fork as *mut c_void, HookSlot::Fork),
    ("/libandroid_runtime.so", b"unshare\0",                new_unshare as *mut c_void, HookSlot::Unshare),
    ("/libandroid_runtime.so", b"selinux_android_setcontext\0",
     new_selinux_android_setcontext as *mut c_void, HookSlot::Setcontext),
    ("/libandroid_runtime.so", b"strdup\0",                 new_strdup as *mut c_void, HookSlot::Strdup),
    ("/libandroid_runtime.so", b"__android_log_close\0",    new_android_log_close as *mut c_void, HookSlot::LogClose),
];
