#![no_std]

extern crate alloc;

mod memory;
mod plt;
mod jni;

use core::ffi::c_void;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(_handle: *mut c_void) {
    jni::hook_jni_env();
}

#[no_mangle]
pub extern "C" fn zygisk_companion_entry(_socket: i32) {}
