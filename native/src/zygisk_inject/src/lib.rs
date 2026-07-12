#![no_std]
#![feature(naked_functions)]
#![allow(non_camel_case_types)]

extern crate alloc;

mod memory;
mod plt;
mod jni_env;
mod hooks;
mod module;
mod unload;
mod module_api;
mod hook_context;
mod fd;
mod ipc;
mod proxy_gen;
mod solist;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe { libc::abort() }
}

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(handle: *mut core::ffi::c_void) {
    hooks::install_hooks(handle);
}

#[no_mangle]
pub extern "C" fn zygisk_companion_entry(_socket: i32) {
}
