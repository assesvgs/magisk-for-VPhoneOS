use alloc::string::ToString;
use alloc::vec::Vec;
use core::ffi::c_void;

#[repr(C)]
pub struct MapInfo {
    pub addr_start: usize,
    pub addr_end: usize,
    pub perms: alloc::string::String,
    pub offset: u64,
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
        if n == 0 { break; }
        if n < 0 { break; }
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
            if line.is_empty() { return None; }

            let mut parts = line.splitn(6, ' ');
            let addr_range = parts.next()?;
            let perms = parts.next()?.to_string();
            let offset_str = parts.next()?;
            let dev_str = parts.next()?;
            let inode_str = parts.next()?;
            let path = parts.next().unwrap_or("").trim().to_string();

            let mut addr_parts = addr_range.splitn(2, '-');
            let addr_start = usize::from_str_radix(addr_parts.next()?, 16).ok()?;
            let addr_end = usize::from_str_radix(addr_parts.next()?, 16).ok()?;
            let offset = u64::from_str_radix(offset_str, 16).ok()?;

            let mut dev_parts = dev_str.splitn(2, ':');
            let dev_major = u64::from_str_radix(dev_parts.next()?, 16).ok()?;
            let dev_minor = u64::from_str_radix(dev_parts.next()?, 16).ok()?;
            let inode: u64 = inode_str.parse().ok()?;

            if inode == 0 || path.is_empty() {
                return None;
            }

            Some(MapInfo {
                addr_start,
                addr_end,
                perms,
                offset,
                dev_major,
                dev_minor,
                inode,
                path,
            })
        })
        .collect()
}

fn get_page_perms(maps: &[MapInfo], page: usize) -> Option<i32> {
    for m in maps {
        if m.addr_start <= page && page < m.addr_end {
            let mut p = 0i32;
            if m.perms.contains('r') { p |= libc::PROT_READ; }
            if m.perms.contains('w') { p |= libc::PROT_WRITE; }
            if m.perms.contains('x') { p |= libc::PROT_EXEC; }
            return Some(p);
        }
    }
    None
}

pub fn find_and_hook(
    maps: &[MapInfo],
    lib_suffix: &str,
    symbol: &[u8],
    hook_fn: *mut c_void,
    orig_fn: *mut *mut c_void,
) -> bool {
    // 1) dlsym 获取原始函数地址
    let target = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr() as *const libc::c_char)
    };
    if target.is_null() {
        return false;
    }

    // 2) 找到该库所有可写映射段（GOT/PLT 在 rw 段）
    let lib_rw: Vec<&MapInfo> = maps.iter()
        .filter(|m| m.path.ends_with(lib_suffix) && m.perms.contains('w'))
        .collect();
    if lib_rw.is_empty() {
        return false;
    }

    // 3) 扫描指针宽度对齐的值，匹配 dlsym 地址
    let ptr_size = core::mem::size_of::<*mut c_void>();
    let align_mask = ptr_size - 1;
    for seg in &lib_rw {
        let mut addr = (seg.addr_start + align_mask) & !align_mask;
        let end = seg.addr_end;
        while addr + ptr_size <= end {
            let val = unsafe { *(addr as *const *mut c_void) };
            if val == target {
                // 找到 GOT 条目
                let page = addr & !0xfff;
                let orig_perms = get_page_perms(maps, page).unwrap_or(libc::PROT_READ);

                // mprotect 为可写（检查返回值！）
                if unsafe {
                    libc::mprotect(page as *mut c_void, 0x1000,
                                   libc::PROT_READ | libc::PROT_WRITE)
                } != 0 {
                    return false;
                }

                // 保存原值
                unsafe { *orig_fn = *(addr as *mut *mut c_void) };

                // 写入 hook 地址
                unsafe { *(addr as *mut *mut c_void) = hook_fn };

                // 恢复权限
                unsafe {
                    libc::mprotect(page as *mut c_void, 0x1000, orig_perms);
                }

                // 记录以便卸载时恢复
                let sym_len = symbol.iter().position(|&b| b == 0).unwrap_or(symbol.len());
                crate::module_api::push_plt_hook(0, 0, addr, orig_perms, &symbol[..sym_len],
                                                 unsafe { *orig_fn });
                return true;
            }
            addr += 8;
        }
    }
    false
}

/// 立即应用——此函数保留以兼容 hooks.rs 调用，实际无操作
pub fn commit_all() -> bool {
    true
}

/// 恢复所有 PLT hook
pub fn restore_all_hooks() -> bool {
    let list = crate::module_api::get_plt_hook_list();
    if list.is_empty() { return true; }
    // 获取当前 maps 用于验证地址有效性
    let maps = scan_maps();
    for entry in list.iter() {
        let addr = entry.addr;
        if addr == 0 { continue; }
        let page = addr & !0xfff;

        // 跳过不再映射的地址
        if get_page_perms(&maps, page).is_none() { continue; }

        // mprotect 为可写
        if unsafe {
            libc::mprotect(page as *mut c_void, 0x1000,
                           libc::PROT_READ | libc::PROT_WRITE)
        } != 0 {
            continue;
        }

        // 恢复原值
        unsafe { *(addr as *mut *mut c_void) = entry.orig; }

        // 恢复原始权限
        let restore = if entry.perms != 0 { entry.perms } else { libc::PROT_READ | libc::PROT_EXEC };
        unsafe {
            libc::mprotect(page as *mut c_void, 0x1000, restore);
        }
    }
    true
}
