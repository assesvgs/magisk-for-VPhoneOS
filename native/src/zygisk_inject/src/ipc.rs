use core::ffi::c_void;

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

pub fn get_magisk_tmp() -> Option<alloc::string::String> {
    let fd = unsafe {
        libc::open(
            b"/proc/self/mountinfo\0".as_ptr() as *const libc::c_char,
            libc::O_RDONLY,
        )
    };
    if fd < 0 {
        return None;
    }

    let mut buf = [0u8; 4096];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
    unsafe { libc::close(fd) };
    if n <= 0 {
        return None;
    }

    let content = &buf[..n as usize];
    let mut pos = 0;
    while pos < content.len() {
        let line_start = pos;
        while pos < content.len() && content[pos] != b'\n' {
            pos += 1;
        }
        let line = &content[line_start..pos];
        pos += 1;

        if line.is_empty() {
            continue;
        }

        let mut field_count = 0;
        let mut mp_start = None;
        let mut mp_end = 0;
        let mut in_field = false;

        for (i, &b) in line.iter().enumerate() {
            if b == b' ' {
                if in_field {
                    in_field = false;
                    if field_count == 5 && mp_start.is_some() {
                        mp_end = i;
                        break;
                    }
                }
            } else if !in_field {
                in_field = true;
                field_count += 1;
                if field_count == 5 {
                    mp_start = Some(i);
                }
            }
        }
        if in_field && field_count == 5 {
            if mp_start.is_some() {
                mp_end = line.len();
            }
        }

        if let Some(start) = mp_start {
            let mount_point = &line[start..mp_end];
            if contains_subslice(mount_point, b".magisk") {
                return alloc::string::String::from_utf8(mount_point.to_vec()).ok();
            }
        }
    }

    None
}

pub fn connect_daemon() -> Option<i32> {
    let tmp = get_magisk_tmp()?;
    let mut path = alloc::string::String::from(tmp);
    path.push_str("/magiskd");
    let c_path = alloc::ffi::CString::new(path).ok()?;

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return None;
    }

    let mut addr: libc::sockaddr_un = unsafe { core::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as u16;
    let bytes = c_path.as_bytes();
    let path_len = bytes.len().min(107);
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            addr.sun_path.as_mut_ptr() as *mut u8,
            path_len,
        );
    }
    let addr_len = core::mem::size_of::<libc::sa_family_t>() + path_len;
    let ret = unsafe {
        libc::connect(
            fd,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            addr_len as libc::socklen_t,
        )
    };
    if ret < 0 {
        unsafe { libc::close(fd) };
        return None;
    }
    Some(fd)
}

/// 跨平台一致的 CMSG_LEN 包装。
/// libc crate 的 CMSG_LEN 在不同目标平台返回类型不同（usize 或 u32），
/// 统一转换为 usize 避免类型错误。
fn cmsg_len(len: usize) -> usize {
    libc::CMSG_LEN(len as u32) as usize
}

pub fn send_fd(sock: i32, fd_to_send: i32) -> bool {
    let mut iov = libc::iovec {
        iov_base: &fd_to_send as *const i32 as *mut c_void,
        iov_len: core::mem::size_of::<i32>(),
    };
    let mut cmsg_buf = [0u8; 32];
    let mut msg: libc::msghdr = unsafe { core::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut c_void;
    msg.msg_controllen = cmsg_buf.len();
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return false;
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = cmsg_len(core::mem::size_of::<i32>());
        core::ptr::write(libc::CMSG_DATA(cmsg) as *mut i32, fd_to_send);
        msg.msg_controllen = libc::CMSG_SPACE(core::mem::size_of::<i32>() as u32) as usize;
    }
    let ret = unsafe { libc::sendmsg(sock, &msg, 0) };
    ret > 0
}

pub fn recv_fds(sock: i32) -> alloc::vec::Vec<i32> {
    let mut data: i32 = 0;
    let mut iov = libc::iovec {
        iov_base: &mut data as *mut i32 as *mut core::ffi::c_void,
        iov_len: core::mem::size_of::<i32>(),
    };
    let mut cmsg_buf = [0u8; 4096];
    let mut msg: libc::msghdr = unsafe { core::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut core::ffi::c_void;
    msg.msg_controllen = cmsg_buf.len();

    let ret = unsafe { libc::recvmsg(sock, &mut msg, 0) };
    if ret < 0 {
        return alloc::vec::Vec::new();
    }

    let mut fds = alloc::vec::Vec::new();
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if !cmsg.is_null()
            && (*cmsg).cmsg_level == libc::SOL_SOCKET
            && (*cmsg).cmsg_type == libc::SCM_RIGHTS
        {
            let payload_len = (*cmsg).cmsg_len - cmsg_len(0);
            let num_fds = payload_len / core::mem::size_of::<i32>();
            let data_ptr = libc::CMSG_DATA(cmsg) as *const i32;
            for i in 0..num_fds {
                fds.push(core::ptr::read(data_ptr.add(i)));
            }
        }
    }
    fds
}

pub fn recv_fd(sock: i32) -> Option<i32> {
    let mut data: i32 = 0;
    let mut iov = libc::iovec {
        iov_base: &mut data as *mut i32 as *mut c_void,
        iov_len: core::mem::size_of::<i32>(),
    };
    let mut cmsg_buf = [0u8; 32];
    let mut msg: libc::msghdr = unsafe { core::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut c_void;
    msg.msg_controllen = cmsg_buf.len();

    let ret = unsafe { libc::recvmsg(sock, &mut msg, 0) };
    if ret < 0 {
        return None;
    }

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return None;
        }
        if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
            let fd = core::ptr::read(libc::CMSG_DATA(cmsg) as *mut i32);
            Some(fd)
        } else {
            None
        }
    }
}

pub fn read_int(fd: i32) -> Option<i32> {
    let mut val: i32 = 0;
    let ptr = &mut val as *mut i32 as *mut c_void;
    let n = unsafe { libc::read(fd, ptr, core::mem::size_of::<i32>()) };
    if n != core::mem::size_of::<i32>() as isize { None } else { Some(val) }
}

pub fn write_int(fd: i32, val: i32) -> bool {
    let val_ne = val.to_ne_bytes();
    let n = unsafe {
        libc::write(
            fd,
            val_ne.as_ptr() as *const c_void,
            core::mem::size_of::<i32>(),
        )
    };
    n == core::mem::size_of::<i32>() as isize
}

pub fn read_string(fd: i32) -> Option<alloc::string::String> {
    let len = read_int(fd)?;
    if len <= 0 || len > 65536 {
        return None;
    }
    let mut buf = alloc::vec![0u8; len as usize];
    let ptr = buf.as_mut_ptr() as *mut c_void;
    let n = unsafe { libc::read(fd, ptr, len as usize) };
    if n != len as isize {
        return None;
    }
    alloc::string::String::from_utf8(buf).ok()
}

pub fn write_string(fd: i32, s: &str) -> bool {
    if !write_int(fd, s.len() as i32) {
        return false;
    }
    let n = unsafe { libc::write(fd, s.as_ptr() as *const c_void, s.len()) };
    n == s.len() as isize
}

/// Daemon IPC 命令码（必须与 lib.rs 中的 ZygiskRequest 枚举的值一致）
/// ZygiskRequest { GetInfo=0, ConnectCompanion=1, GetModDir=2, SulistRootNs=3, RevertUmount=4 }
#[repr(i32)]
enum DaemonCommand {
    RemoteGetInfo = 0,
    ConnectCompanion = 1,
    // 注：值 2 = GetModDir（未被 inject 库直接使用）
    RequestSulist = 3,    // ZygiskRequest::SulistRootNs
    RequestUmount = 4,    // ZygiskRequest::RevertUmount
}

pub fn remote_get_info(uid: i32, process: &str) -> Option<(u32, alloc::vec::Vec<i32>)> {
    let fd = connect_daemon()?;
    if !write_int(fd, DaemonCommand::RemoteGetInfo as i32) {
        return None;
    }
    if !write_int(fd, uid) {
        return None;
    }
    if !write_string(fd, process) {
        return None;
    }
    if !write_int(fd, if cfg!(target_pointer_width = "64") { 64 } else { 32 }) {
        return None;
    }

    let info_flags = read_int(fd)? as u32;
    let fd_count = read_int(fd)?;
    let mut fds = alloc::vec::Vec::new();
    for _ in 0..fd_count {
        if let Some(f) = recv_fd(fd) {
            fds.push(f);
        }
    }
    unsafe { libc::close(fd) };
    Some((info_flags, fds))
}

pub fn request_sulist() -> Option<i32> {
    let fd = connect_daemon()?;
    if !write_int(fd, DaemonCommand::RequestSulist as i32) {
        unsafe { libc::close(fd) };
        return None;
    }
    let result = recv_fd(fd);
    unsafe { libc::close(fd) };
    result
}

pub fn request_umount() -> Option<i32> {
    let fd = connect_daemon()?;
    if !write_int(fd, DaemonCommand::RequestUmount as i32) {
        unsafe { libc::close(fd) };
        return None;
    }
    let result = recv_fd(fd);
    unsafe { libc::close(fd) };
    result
}

pub fn connect_companion(client: i32) -> bool {
    let fd = match connect_daemon() {
        Some(f) => f,
        None => return false,
    };
    if !write_int(fd, DaemonCommand::ConnectCompanion as i32) {
        unsafe { libc::close(fd) };
        return false;
    }
    if !send_fd(fd, client) {
        unsafe { libc::close(fd) };
        return false;
    }
    unsafe { libc::close(fd) };
    true
}
