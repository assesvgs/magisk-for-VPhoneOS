use crate::consts::MODULEROOT;
use crate::daemon::MagiskD;
use crate::ffi::{ZygiskRequest, ZygiskStateFlags};
use crate::socket::{IpcRead, IpcWrite, UnixSocketExt};
use base::{Directory, FsPathBuilder, ResultExt, WriteExt, cstr, libc, raw_cstr, warn};
use std::fmt::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;


const UNMOUNT_MASK: u32 =
    ZygiskStateFlags::ProcessOnDenyList.repr | ZygiskStateFlags::DenyListEnforced.repr;

pub fn zygisk_should_load_module(flags: u32) -> bool {
    flags & UNMOUNT_MASK != UNMOUNT_MASK
        && flags & ZygiskStateFlags::ProcessIsMagiskApp.repr == 0
}

#[derive(Default)]
pub struct ZygiskState {
    sockets: (Option<UnixStream>, Option<UnixStream>),
    start_count: u32,
}

impl ZygiskState {
    fn connect_zygiskd(&mut self, mut client: UnixStream, daemon: &MagiskD) -> Option<()> {
        let is_64_bit: i32 = client.read_decodable().ok()?;
        let slot = if is_64_bit != 0 { &mut self.sockets.1 } else { &mut self.sockets.0 };

        if let Some(s) = slot.as_ref() {
            let fd = s.as_raw_fd();
            let mut pfd = libc::pollfd { fd, events: 0, revents: 0 };
            let poll_ret = unsafe { libc::poll(&mut pfd, 1, 0) };
            if poll_ret == -1 || pfd.revents != 0 {
                *slot = None;
            }
        }

        if let Some(s) = slot.as_mut() {
            s.send_fds(&[client.as_raw_fd()]).ok()?;
        } else {
            let (mut local, remote) = UnixStream::pair().ok()?;
            if unsafe { libc::fork() } == 0 {
                exec_zygiskd(is_64_bit != 0, remote);
            }
            if let Some(module_fds) = daemon.get_module_fds(is_64_bit != 0) {
                local.send_fds(&module_fds).ok()?;
            }
            if local.read_decodable::<i32>().ok()? != 0 {
                warn!("zygisk: zygiskd companion init failed");
                return None;
            }
            local.send_fds(&[client.as_raw_fd()]).ok()?;
            *slot = Some(local);
        }
        Some(())
    }

    pub fn reset(&mut self, restore: bool) {
        if restore {
            self.start_count = 1;
            crate::ffi::set_zygisk_stop_tracing(false);
        } else {
            self.sockets = (None, None);
            self.start_count += 1;
            if self.start_count > 3 {
                warn!("zygote crashed too many times, stop injecting");
                crate::ffi::set_zygisk_stop_tracing(true);
            }
        }
    }
}

fn exec_zygiskd(is_64_bit: bool, remote: UnixStream) {
    unsafe { libc::fcntl(remote.as_raw_fd(), libc::F_SETFD, 0); }

    #[cfg(target_pointer_width = "64")]
    let magisk_name = if is_64_bit { "magisk" } else { "magisk32" };
    #[cfg(target_pointer_width = "32")]
    let magisk_name = "magisk";

    let exe = cstr::buf::default()
        .join_path(crate::ffi::get_magisk_tmp())
        .join_path(magisk_name);
    let mut fd_str = cstr::buf::default();
    write!(fd_str, "{}", remote.as_raw_fd()).ok();

    unsafe {
        libc::execl(
            exe.as_ptr(),
            raw_cstr!(""),
            raw_cstr!("zygisk"),
            raw_cstr!("companion"),
            fd_str.as_ptr(),
            std::ptr::null::<libc::c_char>(),
        );
        libc::exit(-1);
    }
}

pub fn zygisk_handler(daemon: &MagiskD, mut client: UnixStream) {
    let _ = || -> Option<()> {
        let code = ZygiskRequest { repr: client.read_decodable().ok()? };
        match code {
            ZygiskRequest::GetInfo => { daemon.get_process_info(client)?; Some(()) }
            ZygiskRequest::ConnectCompanion => {
                daemon.zygisk.lock().connect_zygiskd(client, daemon)?;
                Some(())
            }
            ZygiskRequest::GetModDir => { daemon.get_mod_dir(client)?; Some(()) }
            ZygiskRequest::SulistRootNs => {
                unsafe {
                    let pid: i32 = client.peer_cred().ok()?.pid?;
                    let child = libc::fork();
                    if child == 0 {
                        crate::ffi::do_mount_magisk(pid);
                        libc::_exit(0);
                    } else if child > 0 {
                        let mut status: i32 = 0;
                        libc::waitpid(child, &mut status as *mut i32, 0);
                        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                            client.write_pod(&(0i32)).ok()?;
                        } else {
                            client.write_pod(&(-1i32)).ok()?;
                        }
                    } else {
                        client.write_pod(&(-1i32)).ok()?;
                    }
                }
                Some(())
            }
            ZygiskRequest::RevertUmount => {
                unsafe {
                    let pid: i32 = client.peer_cred().ok()?.pid?;
                    let child = libc::fork();
                    if child == 0 {
                        crate::mount::revert_unmount(pid);
                        libc::_exit(0);
                    } else if child > 0 {
                        let mut status: i32 = 0;
                        libc::waitpid(child, &mut status as *mut i32, 0);
                        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                            let ns_path = format!("/proc/{}/ns/mnt", pid);
                            client.write_encodable(&ns_path).ok()?;
                        } else {
                            client.write_encodable(&String::new()).ok()?;
                        }
                    } else {
                        client.write_encodable(&String::new()).ok()?;
                    }
                }
                Some(())
            }
            _ => Some(())
        };
        Some(())
    }();
}

impl MagiskD {
    fn get_process_info(&self, mut client: UnixStream) -> Option<()> {
        let uid: i32 = client.read_decodable().ok()?;
        let process: String = client.read_decodable().ok()?;
        let is_64_bit: i32 = client.read_decodable().ok()?;

        let mut flags: u32 = 0;
        unsafe {
            crate::ffi::update_deny_flags(uid, &process, &mut flags);
        }

        if self.get_manager_uid(uid % 100000) == uid {
            flags |= ZygiskStateFlags::ProcessIsMagiskApp.repr;
        }

        if self.uid_granted_root(uid) {
            flags |= ZygiskStateFlags::ProcessGrantedRoot.repr;
        }

        client.write_pod(&flags).ok()?;

        if zygisk_should_load_module(flags) {
            if let Some(fds) = self.get_module_fds(is_64_bit != 0) {
                client.send_fds(&fds).ok()?;
            }
        }

        if uid == 1000 && process == "system_server" {
            let _failed_ids: Vec<i64> = client.read_decodable().ok()?;
        }

        Some(())
    }

    pub fn get_module_fds(&self, is_64_bit: bool) -> Option<Vec<i32>> {
        let module_list = self.module_list.get()?;
        Some(module_list.iter().map(|m| {
            let fd = if is_64_bit { m.z64 } else { m.z32 };
            if fd < 0 { 1 } else { fd }
        }).collect())
    }

    fn get_mod_dir(&self, mut client: UnixStream) -> Option<()> {
        let id: i32 = client.read_decodable().ok()?;
        let module = self.module_list.get()?.get(id as usize)?;
        let dir_path = cstr::buf::default()
            .join_path(MODULEROOT)
            .join_path(&module.name);
        let dir = Directory::open(&dir_path).log().ok()?;
        client.send_fds(&[dir.as_raw_fd()]).ok()?;
        Some(())
    }
}
