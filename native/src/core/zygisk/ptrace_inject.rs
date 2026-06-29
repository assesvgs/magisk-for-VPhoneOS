use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::OpenOptionsExt;
use base::{debug, error, info, libc};

pub struct MapInfo {
    pub start: usize,
    pub end: usize,
    pub perms: u32,
    pub is_private: bool,
    pub offset: u64,
    pub dev: u64,
    pub inode: u64,
    pub path: String,
}

impl MapInfo {
    fn contains(&self, addr: usize) -> bool {
        self.start <= addr && addr < self.end
    }
}

pub fn scan_proc_maps(pid: i32) -> Vec<MapInfo> {
    let path = format!("/proc/{}/maps", if pid == 0 { "self".to_string() } else { pid.to_string() });
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let mut maps = Vec::new();
    for line in reader.lines() {
        if let Ok(line) = line {
            if let Some(map) = parse_map_line(&line) {
                maps.push(map);
            }
        }
    }
    maps
}

fn parse_map_line(line: &str) -> Option<MapInfo> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }

    let addr_range: Vec<&str> = parts[0].split('-').collect();
    if addr_range.len() != 2 {
        return None;
    }
    let start = usize::from_str_radix(addr_range[0], 16).ok()?;
    let end = usize::from_str_radix(addr_range[1], 16).ok()?;

    let perm_str = parts[1];
    let mut perms = 0u32;
    if perm_str.as_bytes().get(0).copied() == Some(b'r') { perms |= libc::PROT_READ as u32; }
    if perm_str.as_bytes().get(1).copied() == Some(b'w') { perms |= libc::PROT_WRITE as u32; }
    if perm_str.as_bytes().get(2).copied() == Some(b'x') { perms |= libc::PROT_EXEC as u32; }
    let is_private = perm_str.as_bytes().get(3).copied() == Some(b'p');

    let offset = u64::from_str_radix(parts[2], 16).ok()?;

    let dev_parts: Vec<&str> = parts[3].split(':').collect();
    let dev = if dev_parts.len() == 2 {
        let major = u64::from_str_radix(dev_parts[0], 16).unwrap_or(0);
        let minor = u64::from_str_radix(dev_parts[1], 16).unwrap_or(0);
        (major << 8) | minor
    } else {
        0u64
    };

    let inode = parts[4].parse::<u64>().ok()?;

    let path = if parts.len() >= 6 {
        parts[5..].join(" ")
    } else {
        String::new()
    };

    Some(MapInfo { start, end, perms, is_private, offset, dev, inode, path })
}

pub fn read_proc_mem(pid: i32, remote_addr: usize, buf: &mut [u8]) -> isize {
    let local_iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    let remote_iov = libc::iovec {
        iov_base: remote_addr as *mut libc::c_void,
        iov_len: buf.len(),
    };
    unsafe { libc::process_vm_readv(pid, &local_iov, 1, &remote_iov, 1, 0) }
}

pub fn write_proc_mem(pid: i32, remote_addr: usize, buf: &[u8]) -> isize {
    let local_iov = libc::iovec {
        iov_base: buf.as_ptr() as *mut libc::c_void,
        iov_len: buf.len(),
    };
    let remote_iov = libc::iovec {
        iov_base: remote_addr as *mut libc::c_void,
        iov_len: buf.len(),
    };
    unsafe { libc::process_vm_writev(pid, &local_iov, 1, &remote_iov, 1, 0) }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "x86", target_arch = "arm")))]
compile_error!("ptrace_inject: unsupported target architecture");

#[cfg(any(
    all(target_os = "android"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86"),
    all(target_os = "linux", target_arch = "arm"),
))]
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct user_regs_struct {
    pub regs: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
}

#[cfg(target_arch = "arm")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct user_regs_struct {
    pub uregs: [u32; 18],
}

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct user_regs_struct {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub rbp: u64, pub rbx: u64, pub r11: u64, pub r10: u64,
    pub r9: u64, pub r8: u64, pub rax: u64, pub rcx: u64,
    pub rdx: u64, pub rsi: u64, pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64, pub cs: u64, pub eflags: u64,
    pub rsp: u64, pub ss: u64,
    pub fs_base: u64, pub gs_base: u64,
    pub ds: u64, pub es: u64, pub fs: u64, pub gs: u64,
}

#[cfg(target_arch = "x86")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct user_regs_struct {
    pub ebx: u32, pub ecx: u32, pub edx: u32, pub esi: u32,
    pub edi: u32, pub ebp: u32, pub eax: u32,
    pub xds: u32, pub xes: u32, pub xfs: u32, pub xgs: u32,
    pub orig_eax: u32,
    pub eip: u32, pub xcs: u32, pub eflags: u32,
    pub esp: u32, pub xss: u32,
}


// Android NDK libc 缺少的 ptrace 常量
const PTRACE_SEIZE: libc::c_int = 0x1401;
const PTRACE_O_EXITKILL: libc::c_uint = 0x00001000;

#[cfg(target_arch = "aarch64")]
mod regs {
    use super::user_regs_struct;
    pub unsafe fn reg_sp(regs: &user_regs_struct) -> usize { regs.sp as usize }
    pub unsafe fn set_reg_sp(regs: &mut user_regs_struct, v: usize) { regs.sp = v as u64; }
    pub unsafe fn reg_ip(regs: &user_regs_struct) -> usize { regs.pc as usize }
    pub unsafe fn set_reg_ip(regs: &mut user_regs_struct, v: usize) { regs.pc = v as u64; }
    pub unsafe fn reg_ret(regs: &user_regs_struct) -> usize { regs.regs[0] as usize }
    pub unsafe fn set_regs_arg(regs: &mut user_regs_struct, i: usize, v: usize) {
        if i < 31 { regs.regs[i] = v as u64; }
    }
    pub unsafe fn set_lr(regs: &mut user_regs_struct, v: usize) { regs.regs[30] = v as u64; }
}

#[cfg(target_arch = "x86_64")]
mod regs {
    use super::user_regs_struct;
    pub unsafe fn reg_sp(regs: &user_regs_struct) -> usize { regs.rsp as usize }
    pub unsafe fn set_reg_sp(regs: &mut user_regs_struct, v: usize) { regs.rsp = v as u64; }
    pub unsafe fn reg_ip(regs: &user_regs_struct) -> usize { regs.rip as usize }
    pub unsafe fn set_reg_ip(regs: &mut user_regs_struct, v: usize) { regs.rip = v as u64; }
    pub unsafe fn reg_ret(regs: &user_regs_struct) -> usize { regs.rax as usize }
    pub unsafe fn set_regs_arg(regs: &mut user_regs_struct, i: usize, v: usize) {
        match i {
            0 => regs.rdi = v as u64,
            1 => regs.rsi = v as u64,
            2 => regs.rdx = v as u64,
            3 => regs.rcx = v as u64,
            4 => regs.r8 = v as u64,
            5 => regs.r9 = v as u64,
            _ => {}
        }
    }
    pub unsafe fn set_lr(_regs: &mut user_regs_struct, _v: usize) {}
}

#[cfg(target_arch = "arm")]
mod regs {
    use super::user_regs_struct;
    pub unsafe fn reg_sp(regs: &user_regs_struct) -> usize { regs.uregs[13] as usize }
    pub unsafe fn set_reg_sp(regs: &mut user_regs_struct, v: usize) { regs.uregs[13] = v as u32; }
    pub unsafe fn reg_ip(regs: &user_regs_struct) -> usize { regs.uregs[15] as usize }
    pub unsafe fn set_reg_ip(regs: &mut user_regs_struct, v: usize) { regs.uregs[15] = v as u32; }
    pub unsafe fn reg_ret(regs: &user_regs_struct) -> usize { regs.uregs[0] as usize }
    pub unsafe fn set_regs_arg(regs: &mut user_regs_struct, i: usize, v: usize) {
        if i < 13 { regs.uregs[i] = v as u32; }
    }
    pub unsafe fn set_lr(_regs: &mut user_regs_struct, _v: usize) {}
}

#[cfg(target_arch = "x86")]
mod regs {
    use super::user_regs_struct;
    pub unsafe fn reg_sp(regs: &user_regs_struct) -> usize { regs.esp as usize }
    pub unsafe fn set_reg_sp(regs: &mut user_regs_struct, v: usize) { regs.esp = v as u32; }
    pub unsafe fn reg_ip(regs: &user_regs_struct) -> usize { regs.eip as usize }
    pub unsafe fn set_reg_ip(regs: &mut user_regs_struct, v: usize) { regs.eip = v as u32; }
    pub unsafe fn reg_ret(regs: &user_regs_struct) -> usize { regs.eax as usize }
    pub unsafe fn set_regs_arg(regs: &mut user_regs_struct, i: usize, v: usize) {
        match i {
            0 => regs.ebx = v as u32,
            1 => regs.ecx = v as u32,
            2 => regs.edx = v as u32,
            3 => regs.esi = v as u32,
            4 => regs.edi = v as u32,
            5 => regs.ebp = v as u32,
            _ => {}
        }
    }
    pub unsafe fn set_lr(_regs: &mut user_regs_struct, _v: usize) {}
}

pub fn get_regs(pid: i32) -> Result<user_regs_struct, nix::Error> {
    let mut regs = unsafe { std::mem::zeroed::<user_regs_struct>() };
    let ret = unsafe {
        libc::ptrace(
            libc::PTRACE_GETREGS,
            pid as libc::pid_t,
            std::ptr::null_mut::<libc::c_void>(),
            &mut regs as *mut _ as *mut libc::c_void,
        )
    };
    if ret == -1 {
        Err(nix::errno::Errno::last())
    } else {
        Ok(regs)
    }
}

pub fn set_regs(pid: i32, regs: &user_regs_struct) -> Result<(), nix::Error> {
    let ret = unsafe {
        libc::ptrace(
            libc::PTRACE_SETREGS,
            pid as libc::pid_t,
            std::ptr::null_mut::<libc::c_void>(),
            regs as *const _ as *mut libc::c_void,
        )
    };
    if ret == -1 {
        Err(nix::errno::Errno::last())
    } else {
        Ok(())
    }
}

fn find_module_base(maps: &[MapInfo], suffix: &str) -> Option<usize> {
    maps.iter().find(|m| m.offset == 0 && m.path.ends_with(suffix)).map(|m| m.start)
}

fn find_module_return_addr(maps: &[MapInfo], suffix: &str) -> Option<usize> {
    maps.iter()
        .find(|m| (m.perms & (libc::PROT_EXEC as u32)) == 0 && m.path.ends_with(suffix))
        .map(|m| m.start)
}

fn find_func_addr(
    local_maps: &[MapInfo],
    remote_maps: &[MapInfo],
    module: &str,
    func: &str,
) -> Option<usize> {
    let module_c = std::ffi::CString::new(module).ok()?;
    let func_c = std::ffi::CString::new(func).ok()?;

    let lib = unsafe { libc::dlopen(module_c.as_ptr(), libc::RTLD_NOW) };
    if lib.is_null() {
        error!("zygisk: failed to open lib {}: dlopen returned null", module);
        return None;
    }
    let sym = unsafe { libc::dlsym(lib, func_c.as_ptr()) as usize };
    if sym == 0 {
        error!("zygisk: failed to find sym {} in {}", func, module);
        unsafe { libc::dlclose(lib); }
        return None;
    }
    debug!("zygisk: sym {}: {:#x}", func, sym);
    unsafe { libc::dlclose(lib); }

    let local_base = find_module_base(local_maps, module)?;
    let remote_base = find_module_base(remote_maps, module)?;
    debug!("zygisk: local base {:#x} remote base {:#x}", local_base, remote_base);
    Some((sym - local_base) + remote_base)
}

fn align_stack(regs: &mut user_regs_struct, preserve: usize) {
    unsafe {
        let sp = regs::reg_sp(regs);
        let aligned = (sp.wrapping_sub(preserve)) & !0xf;
        regs::set_reg_sp(regs, aligned);
    }
}

fn push_string(pid: i32, regs: &mut user_regs_struct, s: &str) -> usize {
    let cstr = std::ffi::CString::new(s).unwrap();
    let bytes = cstr.as_bytes_with_nul();
    let len = bytes.len();
    unsafe {
        let new_sp = regs::reg_sp(regs).wrapping_sub(len);
        regs::set_reg_sp(regs, new_sp);
    }
    align_stack(regs, 0);
    let addr = unsafe { regs::reg_sp(regs) };
    write_proc_mem(pid, addr, bytes);
    debug!("zygisk: pushed string at {:#x}", addr);
    addr
}

pub fn remote_call(
    pid: i32,
    regs: &mut user_regs_struct,
    func_addr: usize,
    return_addr: usize,
    args: &[usize],
) -> usize {
    align_stack(regs, 0);
    debug!("zygisk: remote_call func={:#x} return={:#x} args={}", func_addr, return_addr, args.len());

    #[cfg(target_arch = "aarch64")]
    {
        for (i, &arg) in args.iter().enumerate().take(8) {
            unsafe { regs::set_regs_arg(regs, i, arg); }
        }
        if args.len() > 8 {
            let remain = (args.len() - 8) * std::mem::size_of::<usize>();
            align_stack(regs, remain);
            let stack_addr = unsafe { regs::reg_sp(regs) };
            let buf = unsafe {
                std::slice::from_raw_parts(
                    args[8..].as_ptr() as *const u8,
                    remain,
                )
            };
            write_proc_mem(pid, stack_addr, buf);
        }
        unsafe { regs::set_lr(regs, return_addr); }
    }

    #[cfg(target_arch = "x86_64")]
    {
        for (i, &arg) in args.iter().enumerate().take(6) {
            unsafe { regs::set_regs_arg(regs, i, arg); }
        }
        if args.len() > 6 {
            let remain = (args.len() - 6) * std::mem::size_of::<usize>();
            align_stack(regs, remain);
            let stack_addr = unsafe { regs::reg_sp(regs) };
            let buf = unsafe {
                std::slice::from_raw_parts(
                    args[6..].as_ptr() as *const u8,
                    remain,
                )
            };
            write_proc_mem(pid, stack_addr, buf);
        }
        unsafe {
            let new_sp = regs::reg_sp(regs).wrapping_sub(std::mem::size_of::<usize>());
            regs::set_reg_sp(regs, new_sp);
            write_proc_mem(pid, new_sp, &return_addr.to_ne_bytes());
        }
    }

    unsafe { regs::set_reg_ip(regs, func_addr); }

    if set_regs(pid, regs).is_err() {
        error!("zygisk: remote_call failed to set regs");
        return 0;
    }
    unsafe { libc::ptrace(libc::PTRACE_CONT, pid as libc::pid_t, 0, 0); }

    let wpid = waitpid(Pid::from_raw(pid), Some(WaitPidFlag::__WALL));
    match wpid {
        Ok(WaitStatus::Stopped(tpid, sig)) => {
            let stopped_pid: i32 = tpid.as_raw();
            if get_regs(stopped_pid).is_err() {
                error!("zygisk: remote_call failed to get regs after stop");
                return 0;
            }
            unsafe {
                let ip = regs::reg_ip(regs);
                if sig == nix::sys::signal::Signal::SIGSEGV && ip == return_addr {
                    return regs::reg_ret(regs);
                }
                error!("zygisk: remote_call stopped by {:?} at {:#x}", sig, ip);
            }
        }
        Ok(ws) => {
            error!("zygisk: remote_call unexpected wait status: {:?}", ws);
        }
        Err(e) => {
            error!("zygisk: remote_call waitpid error: {}", e);
        }
    }
    0
}

fn wait_for_trace(pid: i32) -> i32 {
    loop {
        let mut status: i32 = 0;
        let result = unsafe {
            libc::waitpid(pid, &mut status, libc::__WALL)
        };
        if result == -1 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            error!("zygisk: wait_for_trace {} failed: {}", pid, e);
            unsafe { libc::exit(1); }
        }
        let stopped = (status & 0x7f) == 0x7f;
        if !stopped {
            error!("zygisk: process {} not stopped for trace: {:#x}, exit", pid, status);
            unsafe { libc::exit(1); }
        }
        return status;
    }
}

pub fn inject_on_main(pid: i32, lib_path: &str) -> bool {
    let mut regs = match get_regs(pid) {
        Ok(r) => r,
        Err(e) => {
            error!("zygisk: inject_on_main get_regs failed: {}", e);
            return false;
        }
    };
    let regs_backup = regs;

    let maps = scan_proc_maps(pid);
    let arg_ptr = unsafe { regs::reg_sp(&regs) as usize };

    let mut argc_buf = [0u8; std::mem::size_of::<usize>()];
    if read_proc_mem(pid, arg_ptr, &mut argc_buf) <= 0 {
        error!("zygisk: failed to read argc");
        return false;
    }
    let argc = usize::from_ne_bytes(argc_buf[..std::mem::size_of::<usize>()].try_into().unwrap());

    let argv_ptr = arg_ptr + std::mem::size_of::<usize>();
    let envp_ptr = argv_ptr + (argc + 1) * std::mem::size_of::<usize>();

    let mut auxv_ptr = envp_ptr;
    loop {
        let mut ptr_buf = [0u8; std::mem::size_of::<usize>()];
        if read_proc_mem(pid, auxv_ptr, &mut ptr_buf) <= 0 { break; }
        let ptr = usize::from_ne_bytes(ptr_buf[..std::mem::size_of::<usize>()].try_into().unwrap());
        if ptr == 0 { auxv_ptr += std::mem::size_of::<usize>(); break; }
        auxv_ptr += std::mem::size_of::<usize>();
    }

    #[repr(C)]
    struct ElfAuxv {
        a_type: u64,
        a_val: u64,
    }

    let mut entry_addr: usize = 0;
    let mut addr_of_entry_addr: usize = 0;
    let mut current = auxv_ptr;

    loop {
        let mut auxv_buf = [0u8; std::mem::size_of::<ElfAuxv>()];
        if read_proc_mem(pid, current, &mut auxv_buf) <= 0 { break; }
        let auxv: ElfAuxv = unsafe { std::mem::transmute(auxv_buf) };

        const AT_ENTRY: u64 = 9;
        const AT_NULL: u64 = 0;

        if auxv.a_type == AT_ENTRY {
            entry_addr = auxv.a_val as usize;
            addr_of_entry_addr = current + 8;
            debug!("zygisk: entry address {:#x}", entry_addr);
            break;
        }
        if auxv.a_type == AT_NULL { break; }
        current += std::mem::size_of::<ElfAuxv>();
    }

    if entry_addr == 0 {
        error!("zygisk: failed to get AT_ENTRY");
        return false;
    }

    let break_addr: usize = ((-0x05ec1cffi64) as usize & !1usize) | (entry_addr & 1);
    if write_proc_mem(pid, addr_of_entry_addr, &break_addr.to_ne_bytes()) <= 0 {
        error!("zygisk: failed to write break addr");
        return false;
    }

    unsafe { libc::ptrace(libc::PTRACE_CONT, pid as libc::pid_t, 0, 0); }
    let status = wait_for_trace(pid);

    let sig = status & 0x7f;
    if sig != libc::SIGSEGV as i32 {
        error!("zygisk: unexpected signal {} at entry trap", sig);
        return false;
    }

    regs = match get_regs(pid) {
        Ok(r) => r,
        Err(e) => {
            error!("zygisk: get_regs after SIGSEGV failed: {}", e);
            return false;
        }
    };

    let ip = unsafe { regs::reg_ip(&regs) };
    if (ip & !1) != (break_addr & !1) {
        error!("zygisk: stopped at unknown addr {:#x}", ip);
        return false;
    }

    if write_proc_mem(pid, addr_of_entry_addr, &entry_addr.to_ne_bytes()) <= 0 {
        error!("zygisk: failed to restore entry addr");
        return false;
    }

    let maps = scan_proc_maps(pid);
    let local_maps = scan_proc_maps(0);

    let libc_return_addr = match find_module_return_addr(&maps, "libc.so") {
        Some(a) => a,
        None => {
            error!("zygisk: failed to find libc return addr");
            return false;
        }
    };

    let dlopen_addr = match find_func_addr(&local_maps, &maps, "libdl.so", "dlopen") {
        Some(a) => a,
        None => return false,
    };

    let str_addr = push_string(pid, &mut regs, lib_path);
    let remote_handle = remote_call(pid, &mut regs, dlopen_addr, libc_return_addr, &[str_addr, libc::RTLD_NOW as usize]);
    debug!("zygisk: remote dlopen handle = {:#x}", remote_handle);

    if remote_handle == 0 {
        error!("zygisk: dlopen returned null");
        let dlerror_addr = match find_func_addr(&local_maps, &maps, "libdl.so", "dlerror") {
            Some(a) => a,
            None => return false,
        };
        let err_str_addr = remote_call(pid, &mut regs, dlerror_addr, libc_return_addr, &[]);
        if err_str_addr != 0 {
            let strlen_addr = match find_func_addr(&local_maps, &maps, "libc.so", "strlen") {
                Some(a) => a,
                None => return false,
            };
            let err_len = remote_call(pid, &mut regs, strlen_addr, libc_return_addr, &[err_str_addr]);
            if err_len > 0 {
                let mut err_buf = vec![0u8; err_len as usize];
                read_proc_mem(pid, err_str_addr, &mut err_buf);
                if let Ok(s) = String::from_utf8(err_buf) {
                    error!("zygisk: dlerror: {}", s);
                }
            }
        }
        return false;
    }

    let dlsym_addr = match find_func_addr(&local_maps, &maps, "libdl.so", "dlsym") {
        Some(a) => a,
        None => return false,
    };

    let entry_str_addr = push_string(pid, &mut regs, "zygisk_inject_entry");
    let injector_entry = remote_call(pid, &mut regs, dlsym_addr, libc_return_addr, &[remote_handle, entry_str_addr]);
    debug!("zygisk: injector entry = {:#x}", injector_entry);

    if injector_entry == 0 {
        error!("zygisk: injector entry is null");
        return false;
    }

    remote_call(pid, &mut regs, injector_entry, libc_return_addr, &[remote_handle]);

    let mut final_regs = regs_backup;
    unsafe { regs::set_reg_ip(&mut final_regs, entry_addr); }
    debug!("zygisk: invoke entry, restoring pc to {:#x}", entry_addr);

    if set_regs(pid, &final_regs).is_err() {
        error!("zygisk: failed to restore regs");
        return false;
    }

    true
}

pub fn trace_zygote(pid: i32, libpath: &str) -> bool {
    info!("zygisk: start tracing {}", pid);
    let tracee = Pid::from_raw(pid);

    if unsafe { libc::ptrace(PTRACE_SEIZE, tracee.as_raw(), 0, PTRACE_O_EXITKILL as i32) } == -1 {
        error!("zygisk: PTRACE_SEIZE {} failed", pid);
        return false;
    }

    let status = wait_for_trace(pid);
    let sig = status & 0x7f;
    let event = (status >> 16) as u32;
    const PTRACE_EVENT_STOP: u32 = 128;

    if sig != libc::SIGSTOP as i32 || event != PTRACE_EVENT_STOP {
        error!("zygisk: unexpected state after seize: sig={} event={}", sig, event);
        unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
        return false;
    }

    let rstr = format!("/dev/zygisk.{}", rand::random::<u64>());
    {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&rstr)
        {
            Ok(_) => {}
            Err(e) => {
                error!("zygisk: cannot create bind mount file {}: {}", rstr, e);
                unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
                return false;
            }
        }
    }

    let mount_ok = unsafe {
        let src = std::ffi::CString::new(libpath).unwrap();
        let dst = std::ffi::CString::new(rstr.as_str()).unwrap();
        libc::mount(src.as_ptr(), dst.as_ptr(), std::ptr::null(), libc::MS_BIND, std::ptr::null())
    };
    if mount_ok != 0 {
        error!("zygisk: bind mount {} -> {} failed: errno={}", libpath, rstr, mount_ok);
        let _ = std::fs::remove_file(&rstr);
        unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
        return false;
    }

    let injected = inject_on_main(pid, &rstr);

    unsafe {
        let dst = std::ffi::CString::new(rstr.as_str()).unwrap();
        libc::umount2(dst.as_ptr(), libc::MNT_DETACH);
    }
    let _ = std::fs::remove_file(&rstr);

    if !injected {
        error!("zygisk: failed to inject");
        unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
        return false;
    }

    debug!("zygisk: inject done, continue process");
    unsafe { libc::kill(pid, libc::SIGCONT); }

    unsafe { libc::ptrace(libc::PTRACE_CONT, tracee.as_raw(), 0, 0); }
    let status = wait_for_trace(pid);
    let sig2 = status & 0x7f;
    let event2 = (status >> 16) as u32;

    if sig2 == libc::SIGTRAP as i32 && event2 == PTRACE_EVENT_STOP {
        unsafe { libc::ptrace(libc::PTRACE_CONT, tracee.as_raw(), 0, 0); }
        let status3 = wait_for_trace(pid);
        let sig3 = status3 & 0x7f;
        let event3 = (status3 >> 16) as u32;

        if sig3 == libc::SIGCONT as i32 && event3 == 0 {
            debug!("zygisk: received SIGCONT, detaching");
            unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, libc::SIGCONT as i32); }
        } else {
            debug!("zygisk: post-inject state sig={} event={}, detaching", sig3, event3);
            unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
        }
    } else {
        error!("zygisk: unknown state after injection: sig={} event={}", sig2, event2);
        unsafe { libc::ptrace(libc::PTRACE_DETACH, tracee.as_raw(), 0, 0); }
        return false;
    }

    true
}
