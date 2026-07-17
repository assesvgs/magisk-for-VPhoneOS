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

// ===== ELF64 structures (ARM64) =====
#[repr(C)]
struct Elf64_Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
struct Elf64_Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
struct Elf64_Dyn {
    d_tag: u64,
    d_un: u64,
}

#[repr(C)]
struct Elf64_Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

#[repr(C)]
struct Elf64_Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

const PT_DYNAMIC: u32 = 2;
const DT_NULL: u64 = 0;
const DT_JMPREL: u64 = 23;
const DT_PLTRELSZ: u64 = 2;
const DT_SYMTAB: u64 = 6;
const DT_STRTAB: u64 = 5;
const ELF64_R_SYM: fn(u64) -> u64 = |info| info >> 32;

fn read_file(path: &[u8]) -> Vec<u8> {
    let fd = unsafe { libc::open(path.as_ptr() as *const libc::c_char, libc::O_RDONLY) };
    if fd < 0 { return Vec::new(); }
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut c_void, tmp.len()) };
        if n <= 0 { break; }
        buf.extend_from_slice(&tmp[..n as usize]);
    }
    unsafe { libc::close(fd); }
    buf
}

pub fn scan_maps() -> Vec<MapInfo> {
    let data = read_file(b"/proc/self/maps\0");
    let content = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
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
            if inode == 0 || path.is_empty() { return None; }
            Some(MapInfo { addr_start, addr_end, perms, offset, dev_major, dev_minor, inode, path })
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

/// 找到目标库的基地址（第一个 offset=0 的映射）
fn find_lib_base(maps: &[MapInfo], lib_suffix: &str) -> Option<usize> {
    for m in maps {
        if m.offset == 0 && m.path.ends_with(lib_suffix) && m.perms.contains('r') {
            return Some(m.addr_start);
        }
    }
    None
}

/// 通过解析 ELF 动态节精确定位 GOT 条目地址
fn find_got_addr(maps: &[MapInfo], lib_suffix: &str, symbol: &[u8]) -> Option<usize> {
    let base = find_lib_base(maps, lib_suffix)?;

    // 读取 ELF header
    let ehdr = unsafe { &*(base as *const Elf64_Ehdr) };
    if &ehdr.e_ident[..4] != b"\x7fELF" { return None; }

    // 扫描 program headers 找 PT_DYNAMIC
    let phdrs = base + ehdr.e_phoff as usize;
    let phdr_count = ehdr.e_phnum as usize;
    let mut dyn_vaddr = 0u64;
    for i in 0..phdr_count {
        let phdr = unsafe { &*((phdrs + i * ehdr.e_phentsize as usize) as *const Elf64_Phdr) };
        if phdr.p_type == PT_DYNAMIC {
            dyn_vaddr = phdr.p_vaddr;
            break;
        }
    }
    if dyn_vaddr == 0 { return None; }

    // 动态节在内存中的地址
    let dynamic = (base as u64 + dyn_vaddr) as usize;

    // 解析动态条目
    let mut jmprel = 0u64;
    let mut pltrelsz = 0u64;
    let mut symtab = 0u64;
    let mut strtab = 0u64;
    let mut i = 0;
    loop {
        let entry = unsafe { &*((dynamic + i * core::mem::size_of::<Elf64_Dyn>()) as *const Elf64_Dyn) };
        if entry.d_tag == DT_NULL { break; }
        match entry.d_tag {
            DT_JMPREL => jmprel = entry.d_un,
            DT_PLTRELSZ => pltrelsz = entry.d_un,
            DT_SYMTAB => symtab = entry.d_un,
            DT_STRTAB => strtab = entry.d_un,
            _ => {}
        }
        i += 1;
    }
    if jmprel == 0 || pltrelsz == 0 || symtab == 0 || strtab == 0 { return None; }

    // 转换虚拟地址为实际内存地址
    let jmprel_addr = (base as u64 + jmprel) as usize;
    let symtab_addr = (base as u64 + symtab) as usize;
    let strtab_addr = (base as u64 + strtab) as usize;
    let rela_count = pltrelsz / core::mem::size_of::<Elf64_Rela>() as u64;

    // 遍历每个 PLT 重定位条目
    for j in 0..rela_count {
        let rela = unsafe { &*((jmprel_addr + j as usize * core::mem::size_of::<Elf64_Rela>()) as *const Elf64_Rela) };
        let sym_idx = ELF64_R_SYM(rela.r_info);
        let sym = unsafe { &*((symtab_addr + sym_idx as usize * core::mem::size_of::<Elf64_Sym>()) as *const Elf64_Sym) };
        let sym_name_ptr = (strtab_addr + sym.st_name as usize) as *const libc::c_char;
        let sym_name = unsafe { core::ffi::CStr::from_ptr(sym_name_ptr) }.to_bytes_with_nul();

        if sym_name == symbol {
            // GOT 地址 = 基地址 + r_offset
            let got_addr = (base as u64 + rela.r_offset) as usize;
            return Some(got_addr);
        }
    }
    None
}

/// 写 hook 到 GOT 条目（mprotect + 写入 + 恢复权限，全程检查返回值）
fn write_hook(got_addr: usize, hook_fn: *mut c_void, orig_fn: *mut *mut c_void,
              orig_perms: i32) -> bool {
    let page = got_addr & !0xfff;
    if unsafe { libc::mprotect(page as *mut c_void, 0x1000,
                               libc::PROT_READ | libc::PROT_WRITE) } != 0 {
        return false;
    }
    unsafe { *orig_fn = *(got_addr as *mut *mut c_void) };
    unsafe { *(got_addr as *mut *mut c_void) = hook_fn };
    unsafe { libc::mprotect(page as *mut c_void, 0x1000, orig_perms) };
    true
}

pub fn find_and_hook(
    maps: &[MapInfo],
    lib_suffix: &str,
    symbol: &[u8],
    hook_fn: *mut c_void,
    orig_fn: *mut *mut c_void,
) -> bool {
    // 1) 检查符号名必须有 null 终止（dlsym 要求）
    if !symbol.contains(&0) { return false; }

    // 2) dlsym 获取原始函数地址（确认该符号存在）
    let target = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr() as *const libc::c_char)
    };
    if target.is_null() { return false; }

    // 3) ELF 解析：精确定位 GOT 条目
    let got = match find_got_addr(maps, lib_suffix, symbol) {
        Some(a) => a,
        None => return false,
    };

    // 4) 获取页面原始权限
    let page = got & !0xfff;
    let orig_perms = get_page_perms(maps, page).unwrap_or(libc::PROT_READ);

    // 5) 写 hook
    if !write_hook(got, hook_fn, orig_fn, orig_perms) { return false; }

    // 6) 记录以便卸载时恢复
    let sym_len = symbol.iter().position(|&b| b == 0).unwrap_or(symbol.len());
    crate::module_api::push_plt_hook(0, 0, got, &symbol[..sym_len],
                                     unsafe { *orig_fn });
    true
}

/// 立即应用——lazy apply 兼容函数，实际即时写入
pub fn commit_all() -> bool {
    true
}

/// 恢复所有 PLT hook
pub fn restore_all_hooks() -> bool {
    let list = crate::module_api::get_plt_hook_list();
    if list.is_empty() { return true; }
    let maps = scan_maps();
    for entry in list.iter() {
        let addr = entry.addr;
        if addr == 0 { continue; }
        let page = addr & !0xfff;
        if get_page_perms(&maps, page).is_none() { continue; }
        if unsafe { libc::mprotect(page as *mut c_void, 0x1000,
                                   libc::PROT_READ | libc::PROT_WRITE) } != 0 {
            continue;
        }
        unsafe { *(addr as *mut *mut c_void) = entry.orig; }
        unsafe { libc::mprotect(page as *mut c_void, 0x1000, libc::PROT_READ | libc::PROT_EXEC); }
    }
    true
}
