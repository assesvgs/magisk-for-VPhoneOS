use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

pub static SHOULD_UNLOAD: AtomicBool = AtomicBool::new(false);
pub static SELF_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

pub fn save_self_handle(handle: *mut c_void) {
    SELF_HANDLE.store(handle, Ordering::Release);
}

pub fn unhook_functions() -> bool {
    let list = crate::module_api::get_plt_hook_list();
    if list.is_empty() {
        return crate::plt::restore_all_hooks();
    }
    for entry in list.iter() {
        let sym_c = alloc::ffi::CString::new(entry.sym.as_slice()).unwrap_or_default();
        unsafe {
            extern "C" {
                fn zygisk_plt_restore(dev: u64, ino: u64, sym: *const libc::c_char, orig: *mut c_void) -> bool;
            }
            zygisk_plt_restore(entry.dev, entry.ino, sym_c.as_ptr(), entry.orig);
        }
    }
    unsafe {
        extern "C" { fn zygisk_plt_commit() -> bool; }
        zygisk_plt_commit()
    }
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub unsafe extern "C" fn dlclose_self(_handle: *mut c_void) -> ! {
    // _handle 在 x0 中（ARM64 调用约定，第一个参数）
    // dlclose 也读取 x0 作为其参数
    // 所以无需 mov，直接尾调用即可
    core::arch::naked_asm!(
        "b dlclose",
    );
}

#[cfg(not(target_arch = "aarch64"))]
pub unsafe extern "C" fn dlclose_self(handle: *mut c_void) -> ! {
    libc::dlclose(handle);
    // dlclose 失败：无法卸载，立即 abort
    libc::abort()
}
