// 标准库
use std::fmt::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;
use std::sync::atomic::Ordering;

// 外部 crate
use base::libc::STDOUT_FILENO;
use base::{
    Directory, FsPathBuilder, LoggedResult, ResultExt, Utf8CStr, WriteExt, cstr, fork_dont_care,
    libc, log_err, raw_cstr, warn, debug, error,
};
use nix::fcntl::OFlag;

// 内部模块
use crate::consts::MODULEROOT;
use crate::daemon::{MagiskD, to_user_id};
use crate::ffi::{ZygiskRequest, ZygiskStateFlags, get_magisk_tmp, update_deny_flags, switch_mnt_ns, do_mount_magisk};
use crate::mount::revert_unmount;
use crate::resetprop::{get_prop, set_prop};
use crate::socket::{IpcRead, UnixSocketExt};

const NBPROP: &Utf8CStr = cstr!("ro.dalvik.vm.native.bridge");
const ZYGISKLDR: &str = "libzygisk.so";
const UNMOUNT_MASK: u32 =
    ZygiskStateFlags::ProcessOnDenyList.repr | ZygiskStateFlags::DenyListEnforced.repr;

pub fn zygisk_should_load_module(flags: u32) -> bool {
    debug!("zygisk_should_load_module: flags=0x{:x}", flags);
    
    let result = flags & UNMOUNT_MASK != UNMOUNT_MASK && flags & ZygiskStateFlags::ProcessIsMagiskApp.repr == 0;
    
    debug!("zygisk_should_load_module: UNMOUNT_MASK=0x{:x}, result={}", UNMOUNT_MASK, result);
    
    result
}

#[allow(unused_variables)]
fn exec_zygiskd(is_64_bit: bool, remote: UnixStream) {
    // This fd has to survive exec
    unsafe {
        libc::fcntl(remote.as_raw_fd(), libc::F_SETFD, 0);
    }

    // Start building the exec arguments

    #[cfg(target_pointer_width = "64")]
    let magisk = if is_64_bit { "magisk" } else { "magisk32" };

    #[cfg(target_pointer_width = "32")]
    let magisk = "magisk";

    let exe = cstr::buf::new::<64>()
        .join_path(get_magisk_tmp())
        .join_path(magisk);

    let mut fd_str = cstr::buf::new::<16>();
    write!(fd_str, "{}", remote.as_raw_fd()).ok();
    unsafe {
        libc::execl(
            exe.as_ptr(),
            raw_cstr!(""),
            raw_cstr!("zygisk"),
            raw_cstr!("companion"),
            fd_str.as_ptr(),
            ptr::null() as *const libc::c_char,
        );
        libc::exit(-1);
    }
}

#[derive(Default)]
pub struct ZygiskState {
    pub lib_name: String,
    sockets: (Option<UnixStream>, Option<UnixStream>),
    start_count: u32 = 1,
    // clean namespace 缓存（与 kokoro 一致）
    clean_ns64: i32 = -1,
    clean_ns32: i32 = -1,
}

impl ZygiskState {
    fn connect_zygiskd(&mut self, mut client: UnixStream, daemon: &MagiskD) -> LoggedResult<()> {
        debug!("ZygiskState::connect_zygiskd: start");
        
        let is_64_bit: bool = client.read_decodable()?;
        debug!("ZygiskState::connect_zygiskd: is_64_bit={}", is_64_bit);
        let socket = if is_64_bit {
            &mut self.sockets.1
        } else {
            &mut self.sockets.0
        };

        if let Some(fd) = socket {
            // Make sure the socket is still valid
            let mut pfd = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: 0,
                revents: 0,
            };
            if unsafe { libc::poll(&mut pfd, 1, 0) } != 0 || pfd.revents != 0 {
                // Any revent means error
                *socket = None;
            }
        }

        if let Some(fd) = socket {
            fd.send_fds(&[client.as_raw_fd()])?;
        } else {
            // Create a new socket pair and fork zygiskd process
            let (mut local, remote) = UnixStream::pair()?;
            if fork_dont_care() == 0 {
                exec_zygiskd(is_64_bit, remote);
            }
            if let Some(module_fds) = daemon.get_module_fds(is_64_bit) {
                local.send_fds(&module_fds)?;
            }
            if local.read_decodable::<i32>()? != 0 {
                return log_err!();
            }
            local.send_fds(&[client.as_raw_fd()])?;
            *socket = Some(local);
        }
        debug!("ZygiskState::connect_zygiskd: done");
        Ok(())
    }

    pub fn reset(&mut self, mut restore: bool) {
        if restore {
            self.start_count = 1;
        } else {
            self.sockets = (None, None);
            self.start_count += 1;
            // 清理 clean_ns 缓存（与 kokoro 一致）
            self.clean_ns64 = -1;
            self.clean_ns32 = -1;
            if self.start_count > 3 {
                warn!("zygote crashed too many times, rolling-back");
                restore = true;
            }
        }

        if restore {
            self.restore_prop();
        } else {
            self.set_prop();
        }
    }

    pub fn set_prop(&mut self) {
        if !self.lib_name.is_empty() {
            debug!("ZygiskState::set_prop: already set, lib_name={}", self.lib_name);
            return;
        }
        let orig = get_prop(NBPROP);
        debug!("ZygiskState::set_prop: original NBPROP='{}'", orig);
        self.lib_name = if orig.is_empty() || orig == "0" {
            ZYGISKLDR.to_string()
        } else {
            ZYGISKLDR.to_string() + &orig
        };
        debug!("ZygiskState::set_prop: setting NBPROP='{}'", self.lib_name);
        set_prop(NBPROP, Utf8CStr::from_string(&mut self.lib_name));
        // Whether Huawei's Maple compiler is enabled.
        // If so, system server will be created by a special Zygote which ignores the native bridge
        // and make system server out of our control. Avoid it by disabling.
        if get_prop(cstr!("ro.maple.enable")) == "1" {
            debug!("ZygiskState::set_prop: disabling Maple compiler");
            set_prop(cstr!("ro.maple.enable"), cstr!("0"));
        }
        debug!("ZygiskState::set_prop: done");
    }

    pub fn restore_prop(&mut self) {
        let mut orig = "0".to_string();
        if self.lib_name.len() > ZYGISKLDR.len() {
            orig = self.lib_name[ZYGISKLDR.len()..].to_string();
        }
        set_prop(NBPROP, Utf8CStr::from_string(&mut orig));
        self.lib_name.clear();
    }
}

impl MagiskD {
    pub fn zygisk_handler(&self, mut client: UnixStream) {
        let _ = || -> LoggedResult<()> {
            let code = ZygiskRequest {
                repr: client.read_decodable()?,
            };
            debug!("zygisk_handler: request={}", code.repr);
            match code {
                ZygiskRequest::GetInfo => self.get_process_info(client)?,
                ZygiskRequest::ConnectCompanion => self
                    .zygisk
                    .lock()
                    .connect_zygiskd(client, self)
                    .log_with_msg(|w| w.write_str("zygiskd startup error"))?,
                ZygiskRequest::GetModDir => self.get_mod_dir(client)?,
                ZygiskRequest::SulistRootNs => {
                    debug!("zygisk_handler: handling SulistRootNs request");
                    // 从 socket 凭据获取 pid（与 kokoro 一致）
                    let pid = client.peer_cred()
                        .ok()
                        .and_then(|c| c.pid)
                        .unwrap_or(-1);
                    debug!("zygisk_handler: SulistRootNs pid={}", pid);
                    let result = self.mount_magisk_to_remote(pid);
                    debug!("zygisk_handler: SulistRootNs result={}", result);
                    client.write_pod(&result)?;
                }
                ZygiskRequest::RevertUnmount => {
                    debug!("zygisk_handler: handling RevertUnmount request");
                    // 从 socket 凭据获取 pid（与 kokoro 一致）
                    let pid = client.peer_cred()
                        .ok()
                        .and_then(|c| c.pid)
                        .unwrap_or(-1);
                    debug!("zygisk_handler: RevertUnmount pid={}", pid);
                    
                    // 与 kokoro 一致：只有 su_bin_fd >= 0 时才创建 clean namespace
                    // 并缓存结果（每个架构只创建一次）
                    let clean_ns_path = if self.get_su_bin_fd() >= 0 {
                        let ns = self.get_or_create_clean_ns(pid);
                        format!("/proc/{}/fd/{}", std::process::id(), ns)
                    } else {
                        String::new()
                    };
                    
                    debug!("zygisk_handler: RevertUnmount clean_ns_path={}", clean_ns_path);
                    // 写入字符串：先写长度，再写数据
                    let len = clean_ns_path.len() as i32;
                    client.write_pod(&len)?;
                    std::io::Write::write_all(&mut client, clean_ns_path.as_bytes())?;
                }
                _ => {
                    debug!("zygisk_handler: unhandled request={}", code.repr);
                }
            }
            Ok(())
        }();
    }

    fn get_module_fds(&self, is_64_bit: bool) -> Option<Vec<RawFd>> {
        self.module_list.get().map(|module_list| {
            module_list
                .iter()
                .map(|m| if is_64_bit { m.z64 } else { m.z32 })
                // All fds passed over sockets have to be valid file descriptors.
                // To work around this issue, send over STDOUT_FILENO as an indicator of an
                // invalid fd as it will always be /dev/null in magiskd.
                .map(|fd| if fd < 0 { STDOUT_FILENO } else { fd })
                .collect()
        })
    }

    fn get_process_info(&self, mut client: UnixStream) -> LoggedResult<()> {
        let uid: i32 = client.read_decodable()?;
        let process: String = client.read_decodable()?;
        let is_64_bit: bool = client.read_decodable()?;
        debug!("get_process_info: uid={}, process={}, is_64_bit={}", uid, process, is_64_bit);
        let mut flags: u32 = 0;
        update_deny_flags(uid, &process, &mut flags);
        debug!("get_process_info: after update_deny_flags, flags=0x{:x}", flags);
        if self.get_manager_uid(to_user_id(uid)) == uid {
            flags |= ZygiskStateFlags::ProcessIsMagiskApp.repr
        }
        if self.uid_granted_root(uid) {
            flags |= ZygiskStateFlags::ProcessGrantedRoot.repr
        }
        debug!("get_process_info: final flags=0x{:x}", flags);

        // First send flags
        client.write_pod(&flags)?;

        // Next send modules
        if zygisk_should_load_module(flags)
            && let Some(module_fds) = self.get_module_fds(is_64_bit)
        {
            client.send_fds(&module_fds)?;
        }

        // If we're not in system_server, we are done
        if uid != 1000 || process != "system_server" {
            return Ok(());
        }

        // Read all failed modules
        let failed_ids: Vec<i32> = client.read_decodable()?;
        if let Some(module_list) = self.module_list.get() {
            for id in failed_ids {
                let Some(module) = module_list.get(id as usize) else {
                    continue;
                };
                let path = cstr::buf::default()
                    .join_path(MODULEROOT)
                    .join_path(&module.name)
                    .join_path("zygisk");
                // Create the unloaded marker file
                if let Ok(dir) = Directory::open(&path) {
                    dir.open_as_file_at(cstr!("unloaded"), OFlag::O_CREAT | OFlag::O_RDONLY, 0o644)
                        .log()
                        .ok();
                }
            }
        }

        Ok(())
    }

    fn get_mod_dir(&self, mut client: UnixStream) -> LoggedResult<()> {
        let id: i32 = client.read_decodable()?;
        let Some(module) = self
            .module_list
            .get()
            .and_then(|list| list.get(id as usize))
        else {
            return Ok(());
        };
        let dir = cstr::buf::default()
            .join_path(MODULEROOT)
            .join_path(&module.name);
        let fd = dir.open(OFlag::O_RDONLY | OFlag::O_CLOEXEC)?;
        client.send_fds(&[fd.as_raw_fd()])?;
        Ok(())
    }

    fn mount_magisk_to_remote(&self, pid: i32) -> i32 {
        unsafe {
            let child = libc::fork();
            if child == 0 {
                // 子进程：执行 Magisk 挂载
                do_mount_magisk(pid);
                libc::_exit(0);
            } else if child > 0 {
                // 父进程：等待子进程完成
                libc::waitpid(child, std::ptr::null_mut(), 0);
                0 // 成功
            } else {
                -1 // 失败
            }
        }
    }

    fn get_clean_ns_path(&self, pid: i32) -> String {
        unsafe {
            let mut pipe_fd = [0i32; 2];
            libc::pipe(pipe_fd.as_mut_ptr());

            let child = libc::fork();
            if child == 0 {
                // 子进程
                switch_mnt_ns(pid);
                libc::unshare(libc::CLONE_NEWNS);
                revert_unmount(-1);
                // 通知父进程已完成
                let mut buf = 0i32;
                libc::write(pipe_fd[1], &mut buf as *mut i32 as *const libc::c_void, 4);
                // 等待父进程读取
                libc::read(pipe_fd[0], &mut buf as *mut i32 as *mut libc::c_void, 4);
                libc::_exit(0);
            } else {
                // 父进程
                let mut buf = 0i32;
                // 等待子进程完成
                libc::read(pipe_fd[0], &mut buf as *mut i32 as *mut libc::c_void, 4);

                // 获取子进程的 namespace fd
                let ns_path = format!("/proc/{}/ns/mnt\0", child);
                let clean_ns = libc::open(ns_path.as_ptr() as *const libc::c_char, libc::O_RDONLY);

                // 通知子进程可以退出
                libc::write(pipe_fd[1], &mut buf as *mut i32 as *const libc::c_void, 4);
                libc::close(pipe_fd[0]);
                libc::close(pipe_fd[1]);

                // 等待子进程退出
                libc::waitpid(child, std::ptr::null_mut(), 0);

                // 返回 namespace 路径
                if clean_ns >= 0 {
                    let result = format!("/proc/{}/fd/{}", std::process::id(), clean_ns);
                    // 不关闭 clean_ns，因为客户端需要使用它
                    result
                } else {
                    String::new()
                }
            }
        }
    }
}

// FFI to C++
impl MagiskD {
    pub fn zygisk_enabled(&self) -> bool {
        self.zygisk_enabled.load(Ordering::Acquire)
    }

    // 获取 su_bin_fd（与 kokoro 一致）
    fn get_su_bin_fd(&self) -> i32 {
        crate::ffi::get_su_bin_fd()
    }

    // 判断进程是否为 64 位（与 kokoro 的 get_exe + str_ends(buf, "64") 一致）
    fn is_64_bit_process(pid: i32) -> bool {
        let exe_path = format!("/proc/{}/exe\0", pid);
        let mut buf = [0u8; 256];
        unsafe {
            let len = libc::readlink(
                exe_path.as_ptr() as *const libc::c_char,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len() - 1,
            );
            if len > 0 {
                let exe = std::str::from_utf8_unchecked(&buf[..len as usize]);
                exe.ends_with("64")
            } else {
                true // 默认假设 64 位
            }
        }
    }

    // 获取或创建 clean namespace（带缓存，与 kokoro 一致）
    fn get_or_create_clean_ns(&self, pid: i32) -> i32 {
        let mut zygisk = self.zygisk.lock();
        let is_64_bit = Self::is_64_bit_process(pid);
        
        let cached_ns = if is_64_bit {
            &mut zygisk.clean_ns64
        } else {
            &mut zygisk.clean_ns32
        };

        if *cached_ns < 0 {
            *cached_ns = self.create_clean_ns(pid);
        }
        *cached_ns
    }

    // 创建 clean namespace（与 kokoro 的 get_clean_ns 逻辑一致）
    fn create_clean_ns(&self, pid: i32) -> i32 {
        unsafe {
            let mut pipe_fd = [0i32; 2];
            libc::pipe(pipe_fd.as_mut_ptr());

            let child = libc::fork();
            if child == 0 {
                // 子进程
                switch_mnt_ns(pid);
                libc::unshare(libc::CLONE_NEWNS);
                revert_unmount(-1);
                // 通知父进程已完成
                let mut buf = 0i32;
                libc::write(pipe_fd[1], &mut buf as *mut i32 as *const libc::c_void, 4);
                // 等待父进程读取
                libc::read(pipe_fd[0], &mut buf as *mut i32 as *mut libc::c_void, 4);
                libc::_exit(0);
            } else if child > 0 {
                // 父进程：fork 成功
                let mut buf = 0i32;
                // 等待子进程完成
                libc::read(pipe_fd[0], &mut buf as *mut i32 as *mut libc::c_void, 4);

                // 获取子进程的 namespace fd
                let ns_path = format!("/proc/{}/ns/mnt\0", child);
                let clean_ns = libc::open(ns_path.as_ptr() as *const libc::c_char, libc::O_RDONLY);

                // 通知子进程可以退出
                libc::write(pipe_fd[1], &mut buf as *mut i32 as *const libc::c_void, 4);
                libc::close(pipe_fd[0]);
                libc::close(pipe_fd[1]);

                // 等待子进程退出
                libc::waitpid(child, std::ptr::null_mut(), 0);

                clean_ns
            } else {
                // fork 失败
                error!("create_clean_ns: fork failed, errno={}", *libc::__errno());
                libc::close(pipe_fd[0]);
                libc::close(pipe_fd[1]);
                -1
            }
        }
    }
}
