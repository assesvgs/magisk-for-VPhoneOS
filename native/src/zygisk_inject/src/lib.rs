#![no_std]
#![feature(naked_functions)]
#![allow(non_camel_case_types)]

extern crate alloc;

mod memory;
mod platform;
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

/// Build a NUL-terminated byte vector for `/proc/self/fd/{fd}` path.
fn fd_path_bytes(fd: i32) -> alloc::vec::Vec<u8> {
    let prefix = b"/proc/self/fd/";
    let mut bytes = alloc::vec::Vec::with_capacity(prefix.len() + 12);
    bytes.extend_from_slice(prefix);

    let mut n = fd;
    if n < 0 {
        bytes.push(b'-');
        n = -n;
    }
    let mut digits = alloc::vec::Vec::with_capacity(12);
    if n == 0 {
        digits.push(b'0');
    } else {
        while n > 0 {
            digits.push(b'0' + (n % 10) as u8);
            n /= 10;
        }
        digits.reverse();
    }
    bytes.extend(digits);
    bytes.push(0);
    bytes
}

/// Zygisk companion daemon entry point (Rust reimplementation of `main.cpp::zygiskd()`).
///
/// Protocol (compatible with C++ `zygiskd()` and `daemon.rs::connect_zygiskd()`):
/// 1. Read all module FDs from the socket (sent in one `sendmsg` by magiskd)
/// 2. For each regular-file FD: `dlopen("/proc/self/fd/N", RTLD_LAZY)` + `dlsym("zygisk_companion_entry")`
/// 3. Send ack (`write_int(socket, 0)`)
/// 4. Poll loop: `poll` → `recv_fd` (client) → `read_int` (module_id) → dispatch → fstat close guard
///
/// On VPhoneOS: no-op (companion is handled by C++ `zygiskd()` in the magisk binary).
#[no_mangle]
pub extern "C" fn zygisk_companion_entry(socket: i32) {
    // VPhoneOS: no-op; the C++ zygiskd() handles companion duties
    if crate::solist::is_vphoneos() {
        return;
    }

    // Validate root and socket validity
    if unsafe { libc::getuid() != 0 } {
        return;
    }
    if unsafe { libc::fcntl(socket, libc::F_GETFD) } < 0 {
        return;
    }

    // 1. Receive all module FDs from magiskd
    let fds = crate::ipc::recv_fds(socket);
    if fds.is_empty() {
        return;
    }

    // 2. Load each module and resolve its companion entry
    type CompanionFn = unsafe extern "C" fn(i32);
    let mut companions: alloc::vec::Vec<Option<CompanionFn>> =
        alloc::vec::Vec::with_capacity(fds.len());

    for &fd in &fds {
        let mut entry: Option<CompanionFn> = None;
        let mut st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } == 0
            && crate::platform::is_regular_file(&st)
        {
            let path_bytes = fd_path_bytes(fd);
            let c_path = unsafe {
                core::ffi::CStr::from_bytes_with_nul_unchecked(&path_bytes)
            };
            let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_LAZY) };
            if !handle.is_null() {
                let sym = unsafe {
                    libc::dlsym(
                        handle,
                        b"zygisk_companion_entry\0".as_ptr() as *const libc::c_char,
                    )
                };
                if !sym.is_null() {
                    entry = Some(unsafe { core::mem::transmute(sym) });
                }
            }
        }
        companions.push(entry);
        unsafe { libc::close(fd) };
    }

    // 3. Send ack
    crate::ipc::write_int(socket, 0);

    // 4. Poll loop: accept client connections and dispatch to modules
    let mut pfd = libc::pollfd {
        fd: socket,
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        let ret = unsafe { libc::poll(&mut pfd, 1, -1) };
        if ret < 0 {
            break;
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            break;
        }

        let client = match crate::ipc::recv_fd(socket) {
            Some(fd) => fd,
            None => break,
        };

        let module_id = match crate::ipc::read_int(client) {
            Some(id) => id,
            None => {
                unsafe { libc::close(client) };
                continue;
            }
        };

        if module_id >= 0 && (module_id as usize) < companions.len() {
            if let Some(entry_fn) = companions[module_id as usize] {
                let mut s1: libc::stat = unsafe { core::mem::zeroed() };
                unsafe { libc::fstat(client, &mut s1) };

                unsafe { entry_fn(client) };

                let mut s2: libc::stat = unsafe { core::mem::zeroed() };
                if unsafe { libc::fstat(client, &mut s2) } == 0
                    && s1.st_dev == s2.st_dev
                    && s1.st_ino == s2.st_ino
                {
                    unsafe { libc::close(client) };
                }
                continue;
            }
        }
        unsafe { libc::close(client) };
    }
}
