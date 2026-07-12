#![no_std]

extern crate alloc;

mod memory;
mod plt;
mod jni;
mod hooks;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(_handle: *mut core::ffi::c_void) {
    hooks::hook_plt();
}

#[no_mangle]
pub extern "C" fn zygisk_companion_entry(_socket: i32) {}
