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
        let n = unsafe {
            libc::read(fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len())
        };
        if n <= 0 {
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

    let mut result = alloc::vec::Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(6, ' ');
        let addrs = match parts.next() {
            Some(a) => a,
            None => continue,
        };
        let _perms = match parts.next() {
            Some(p) => p,
            None => continue,
        };
        let _offset = match parts.next() {
            Some(o) => o,
            None => continue,
        };
        let dev_str = match parts.next() {
            Some(d) => d,
            None => continue,
        };
        let inode_str = match parts.next() {
            Some(i) => i,
            None => continue,
        };
        let path = parts.next().unwrap_or("").trim().to_string();

        let dev_parts: alloc::vec::Vec<&str> = dev_str.split(':').collect();
        if dev_parts.len() != 2 {
            continue;
        }
        let dev_major = u64::from_str_radix(dev_parts[0], 16).unwrap_or(0);
        let dev_minor = u64::from_str_radix(dev_parts[1], 16).unwrap_or(0);
        let inode = inode_str.parse::<u64>().unwrap_or(0);
        if inode == 0 || path.is_empty() {
            continue;
        }
        result.push(MapInfo {
            dev_major,
            dev_minor,
            inode,
            path,
        });
    }
    result
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
}

pub fn find_and_hook(
    maps: &[MapInfo],
    lib_suffix: &str,
    symbol: &[u8],
    hook_fn: *mut c_void,
    orig_fn: *mut *mut c_void,
) -> bool {
    for map in maps {
        if map.path.ends_with(lib_suffix) {
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
                return true;
            }
        }
    }
    false
}

pub fn commit_all() -> bool {
    unsafe { zygisk_plt_commit() }
}
