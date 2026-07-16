use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

pub static SHOULD_UNLOAD: AtomicBool = AtomicBool::new(false);
pub static SELF_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

pub fn save_self_handle(handle: *mut c_void) {
    SELF_HANDLE.store(handle, Ordering::Release);
}

pub fn unhook_functions() -> bool {
    crate::plt::restore_all_hooks()
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub unsafe extern "C" fn dlclose_self(_handle: *mut c_void) -> ! {
    // _handle 在 x0 中（ARM64 调用约定，第一个参数）
    // dlclose 也读取 x0 作为其参数，所以无需 mov，直接尾调用
    core::arch::naked_asm!(
        "b dlclose",
    );
}

#[cfg(target_arch = "arm")]
#[unsafe(naked)]
pub unsafe extern "C" fn dlclose_self(_handle: *mut c_void) -> ! {
    // _handle 在 r0 中（ARM32 调用约定，第一个参数），dlclose 也读取 r0
    // 32-bit 设备（armeabi-v7a）也需要支持自卸载
    core::arch::naked_asm!(
        "b dlclose",
    );
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
pub unsafe extern "C" fn dlclose_self(handle: *mut c_void) -> ! {
    libc::dlclose(handle);
    // dlclose 失败：无法卸载，立即 abort
    libc::abort()
}
