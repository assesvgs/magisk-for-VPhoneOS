#![no_std]

extern crate alloc;

mod memory;
mod plt;

use core::ffi::c_void;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(_handle: *mut c_void) {
    let maps = plt::scan_maps();
    if maps.is_empty() {
        return;
    }
    plt::find_and_hook(
        &maps,
        "/libandroid_runtime.so",
        b"fork\0",
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    plt::commit_all();
}

#[no_mangle]
pub extern "C" fn zygisk_companion_entry(_socket: i32) {}
