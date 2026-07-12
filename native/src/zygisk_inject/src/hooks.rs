use core::ffi::c_void;

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

fn orig_ptr(slot: HookSlot) -> *mut *mut c_void {
    unsafe { &mut ORIG_FUNCS[slot as usize] as *mut *mut c_void }
}

fn load_orig(slot: HookSlot) -> *mut c_void {
    unsafe { ORIG_FUNCS[slot as usize] }
}

extern "C" fn new_fork() -> i32 {
    let f: ForkFn = unsafe { core::mem::transmute(load_orig(HookSlot::Fork)) };
    unsafe { f() }
}

extern "C" fn new_unshare(flags: i32) -> i32 {
    let f: UnshareFn = unsafe { core::mem::transmute(load_orig(HookSlot::Unshare)) };
    unsafe { f(flags) }
}

extern "C" fn new_selinux_android_setcontext(
    uid: u32, is_system_server: i32,
    seinfo: *const libc::c_char, pkgname: *const libc::c_char,
) -> i32 {
    let f: SetcontextFn = unsafe { core::mem::transmute(load_orig(HookSlot::Setcontext)) };
    unsafe { f(uid, is_system_server, seinfo, pkgname) }
}

extern "C" fn new_strdup(s: *const libc::c_char) -> *mut libc::c_char {
    let f: StrdupFn = unsafe { core::mem::transmute(load_orig(HookSlot::Strdup)) };
    unsafe { f(s) }
}

extern "C" fn new_android_log_close() {
    let f: LogCloseFn = unsafe { core::mem::transmute(load_orig(HookSlot::LogClose)) };
    unsafe { f() }
}

extern "C" fn new_dlclose(handle: *mut c_void) -> i32 {
    let f: DlcloseFn = unsafe { core::mem::transmute(load_orig(HookSlot::Dlclose)) };
    unsafe { f(handle) }
}

pub fn hook_plt() {
    let maps = crate::plt::scan_maps();
    if maps.is_empty() {
        return;
    }

    let nativebridge: &[(&str, &[u8], *mut c_void, HookSlot)] = &[
        ("/libnativebridge.so", b"dlclose\0", new_dlclose as *mut c_void, HookSlot::Dlclose),
    ];
    let android_runtime: &[(&str, &[u8], *mut c_void, HookSlot)] = &[
        ("/libandroid_runtime.so", b"fork\0",                    new_fork as *mut c_void, HookSlot::Fork),
        ("/libandroid_runtime.so", b"unshare\0",                new_unshare as *mut c_void, HookSlot::Unshare),
        ("/libandroid_runtime.so", b"selinux_android_setcontext\0",
         new_selinux_android_setcontext as *mut c_void, HookSlot::Setcontext),
        ("/libandroid_runtime.so", b"strdup\0",                 new_strdup as *mut c_void, HookSlot::Strdup),
        ("/libandroid_runtime.so", b"__android_log_close\0",    new_android_log_close as *mut c_void, HookSlot::LogClose),
    ];

    for &(lib, sym, hook, slot) in nativebridge.iter().chain(android_runtime.iter()) {
        crate::plt::find_and_hook(&maps, lib, sym, hook, orig_ptr(slot));
    }

    crate::plt::commit_all();
}
