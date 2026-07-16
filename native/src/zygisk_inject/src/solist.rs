use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static NEEDS_HIDE: AtomicBool = AtomicBool::new(true);

fn read_file(path: &[u8]) -> Vec<u8> {
    let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDONLY) };
    if fd < 0 {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut c_void, tmp.len()) };
        if n == 0 {
            break;
        }
        if n < 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
    }
    unsafe { libc::close(fd); }
    buf
}

pub fn is_vphoneos() -> bool {
    unsafe { libc::access(b"/share\0".as_ptr() as *const libc::c_char, libc::F_OK) == 0 }
}

pub fn initialize() -> bool {
    if INITIALIZED.load(Ordering::Relaxed) {
        return NEEDS_HIDE.load(Ordering::Relaxed);
    }
    let vphone = is_vphoneos();
    NEEDS_HIDE.store(!vphone, Ordering::Relaxed);
    INITIALIZED.store(true, Ordering::Relaxed);
    !vphone
}

pub fn hide_modules() {
    if !initialize() {
        return;
    }

    let data = read_file(b"/proc/self/maps\0");
    let content = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return,
    };

    struct MapEntry {
        start: usize,
        end: usize,
        perms: i32,
    }

    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.splitn(6, ' ');
        let addr_range = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let perms_str = match parts.next() {
            Some(s) => s,
            None => continue,
        };
        let _offset = parts.next();
        let _dev = parts.next();
        let _inode = parts.next();
        let path = match parts.next() {
            Some(s) => s.trim(),
            None => continue,
        };

        if !path.contains("/memfd:jit-zygisk-cache") && !path.contains("/modules/") {
            continue;
        }

        let mut addr_parts = addr_range.splitn(2, '-');
        let start_str = match addr_parts.next() {
            Some(s) => s,
            None => continue,
        };
        let end_str = match addr_parts.next() {
            Some(s) => s,
            None => continue,
        };
        let start = match usize::from_str_radix(start_str, 16) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let end = match usize::from_str_radix(end_str, 16) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let mut perms = 0i32;
        for c in perms_str.chars() {
            match c {
                'r' => perms |= libc::PROT_READ,
                'w' => perms |= libc::PROT_WRITE,
                'x' => perms |= libc::PROT_EXEC,
                _ => {}
            }
        }

        entries.push(MapEntry { start, end, perms });
    }

    for entry in &entries {
        let size = entry.end - entry.start;
        let addr = entry.start as *mut c_void;

        let copy = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };

        if copy == libc::MAP_FAILED {
            continue;
        }

        if (entry.perms & libc::PROT_READ) == 0 {
            unsafe { libc::mprotect(addr, size, libc::PROT_READ); }
        }

        unsafe {
            ptr::copy_nonoverlapping(addr as *const u8, copy as *mut u8, size);
        }

        // mremap(2) — 使用 syscall 直接调用而非 libc 包装（Android 目标可能不导出此符号）
        // SYS_mremap: aarch64=25, arm/x86=163, x86_64=25
        // libc::syscall 接受 c_long 参数（32-bit=32, 64-bit=64）
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        const SYS_MREMAP: libc::c_long = 25;
        #[cfg(any(target_arch = "arm", target_arch = "x86"))]
        const SYS_MREMAP: libc::c_long = 163;
        const MREMAP_MAYMOVE: libc::c_long = 1;
        const MREMAP_FIXED: libc::c_long = 2;
        let result = unsafe {
            libc::syscall(
                SYS_MREMAP,
                copy as libc::c_long,
                size as libc::c_long,
                size as libc::c_long,
                MREMAP_MAYMOVE | MREMAP_FIXED,
                addr as libc::c_long,
            ) as *mut c_void
        };

        if result == libc::MAP_FAILED {
            unsafe { libc::munmap(copy, size); }
            continue;
        }

        unsafe { libc::mprotect(addr, size, entry.perms); }
    }
}
