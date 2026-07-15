use alloc::string::ToString;
use core::ffi::c_void;

#[repr(C)]
pub struct MapInfo {
    pub dev_major: u64,
    pub dev_minor: u64,
    pub inode: u64,
    pub path: alloc::string::String,
}

fn read_file(path: &[u8]) -> alloc::vec::Vec<u8> {
    let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDONLY) };
    if fd < 0 {
        return alloc::vec::Vec::new();
    }
    let mut buf = alloc::vec::Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut c_void, tmp.len()) };
        if n == 0 { break; }  // EOF
        if n < 0 {
            // read error — 不是 EOF，但继续可能导致无限循环，break
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
    }
    unsafe { libc::close(fd); }
    buf
}

pub fn scan_maps() -> alloc::vec::Vec<MapInfo> {
    let data = read_file(b"/proc/self/maps\0");
    let content = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return alloc::vec::Vec::new(),
    };

    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(6, ' ');
            let _ = parts.next()?; // addr range
            let _ = parts.next()?; // perms
            let _ = parts.next()?; // offset
            let dev_str = parts.next()?;
            let inode_str = parts.next()?;
            let path = parts.next().unwrap_or("").trim().to_string();

            let mut dev_parts = dev_str.splitn(2, ':');
            let dev_major = u64::from_str_radix(dev_parts.next()?, 16).ok()?;
            let dev_minor = u64::from_str_radix(dev_parts.next()?, 16).ok()?;
            let inode: u64 = inode_str.parse().ok()?;

            if inode == 0 || path.is_empty() {
                return None;
            }

            Some(MapInfo {
                dev_major,
                dev_minor,
                inode,
                path,
            })
        })
        .collect()
}

extern "C" {
    fn zygisk_plt_register(
        dev: u64,
        ino: u64,
        symbol: *const libc::c_char,
        hook: *mut c_void,
        orig: *mut *mut c_void,
    ) -> bool;
    fn zygisk_plt_commit() -> bool;
    fn zygisk_plt_restore(dev: u64, ino: u64, symbol: *const libc::c_char, orig: *mut c_void) -> bool;
}

pub fn find_and_hook(
    maps: &[MapInfo],
    lib_suffix: &str,
    symbol: &[u8],
    hook_fn: *mut c_void,
    orig_fn: *mut *mut c_void,
) -> bool {
    for map in maps {
        if !map.path.ends_with(lib_suffix) {
            continue;
        }
        let ok = unsafe {
            zygisk_plt_register(
                (map.dev_major << 20) | map.dev_minor,
                map.inode,
                symbol.as_ptr() as *const libc::c_char,
                hook_fn,
                orig_fn,
            )
        };
        if ok {
            let dev = (map.dev_major << 20) | map.dev_minor;
            let orig_val = unsafe { *orig_fn };
            // 去掉末尾 null 字节
            let sym_len = symbol.iter().position(|&b| b == 0).unwrap_or(symbol.len());
            crate::module_api::push_plt_hook(dev, map.inode, &symbol[..sym_len], orig_val);
            return true;
        }
    }
    false
}

pub fn commit_all() -> bool {
    unsafe { zygisk_plt_commit() }
}

pub fn restore_all_hooks() -> bool {
    let list = crate::module_api::get_plt_hook_list();
    if list.is_empty() { return true; }
    for entry in list.iter() {
        let sym_c = alloc::ffi::CString::new(entry.sym.as_slice()).unwrap_or_default();
        unsafe {
            zygisk_plt_restore(entry.dev, entry.ino, sym_c.as_ptr(), entry.orig);
        }
    }
    unsafe { zygisk_plt_commit() }
}
