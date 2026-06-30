use crate::ffi::get_magisk_tmp;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use base::{error, info, libc};

const PTRACE_SEIZE: libc::c_int = 0x1401;
const PTRACE_O_TRACEFORK: libc::c_int = 0x00000002;
const PTRACE_O_TRACEEXEC: libc::c_int = 0x00000008;

pub static STOP_TRACE_ZYGOTE: AtomicBool = AtomicBool::new(false);
pub static PTRACE_SEIZE_FAILED: AtomicBool = AtomicBool::new(false);

pub fn start_zygisk() {
    std::thread::spawn(|| {
        init_monitor();
    });
}

fn is_zygote(exe_path: &str) -> bool {
    exe_path.contains("app_process")
}

fn scan_zygote() -> Option<Pid> {
    let dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return None,
    };
    for entry in dir.flatten() {
        let pid_str = entry.file_name();
        let pid: i32 = match pid_str.to_str().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        if pid < 2 { continue; }
        let stat = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let after_close = match stat.rfind(')') {
            Some(i) => &stat[i+1..],
            None => continue,
        };
        let fields: Vec<&str> = after_close.split_whitespace().collect();
        let ppid: i32 = match fields.get(1).and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        if ppid != 1 { continue; }
        let cmdline = match std::fs::read_to_string(format!("/proc/{}/cmdline", pid)) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if cmdline.contains("zygote") {
            return Some(Pid::from_raw(pid));
        }
    }
    None
}

fn fallback_to_denylist() {
    PTRACE_SEIZE_FAILED.store(true, Ordering::Release);
    error!("zygisk: init_monitor fallback to denylist mode");
}

pub fn init_monitor() {
    let init_pid = Pid::from_raw(1);
    let opts = PTRACE_O_TRACEFORK as i32;
    let mut seized_init = false;
    let mut attached_via_fallback = false;
    let mut process: HashSet<Pid> = HashSet::new();
    let mut is_fallback = false;

    if unsafe { libc::ptrace(PTRACE_SEIZE, init_pid.as_raw(), 0, opts) } == -1 {
        error!("zygisk: PTRACE_SEIZE init(1) failed, falling back to polling");
        is_fallback = true;
    } else {
        info!("zygisk: init_monitor started, tracing PID 1");
        seized_init = true;
    }

    let mut poll_count = 0;
    loop {
        if STOP_TRACE_ZYGOTE.load(Ordering::Acquire) {
            if seized_init {
                unsafe { libc::ptrace(libc::PTRACE_DETACH, init_pid.as_raw(), 0, 0); }
            }
            break;
        }

        if is_fallback {
            if let Some(zygote_pid) = scan_zygote() {
                info!("zygisk: fallback: found zygote PID={}, attaching", zygote_pid);
                if unsafe { libc::ptrace(libc::PTRACE_ATTACH, zygote_pid.as_raw(), 0, 0) } == -1 {
                    error!("zygisk: fallback: PTRACE_ATTACH zygote failed");
                    fallback_to_denylist();
                    break;
                }
                let _ = waitpid(Some(zygote_pid), Some(WaitPidFlag::__WALL));
                unsafe {
                    libc::ptrace(
                        libc::PTRACE_SETOPTIONS,
                        zygote_pid.as_raw(),
                        0,
                        PTRACE_O_TRACEEXEC as i32,
                    );
                }
                process.insert(zygote_pid);
                unsafe { libc::ptrace(libc::PTRACE_CONT, zygote_pid.as_raw(), 0, 0); }
                attached_via_fallback = true;
                is_fallback = false;
                info!("zygisk: fallback: attached to zygote, entering main loop");
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            poll_count += 1;
            if poll_count > 120 {
                error!("zygisk: fallback: zygote not found after 30s, giving up");
                fallback_to_denylist();
                break;
            }
            continue;
        }

        match waitpid(None, Some(WaitPidFlag::__WALL | WaitPidFlag::__WNOTHREAD)) {
            Ok(WaitStatus::PtraceEvent(tpid, sig, event)) => {
                if seized_init && tpid == init_pid {
                    if event == 0x1 {
                        let mut _ev: u64 = 0;
                        unsafe {
                            libc::ptrace(
                                libc::PTRACE_GETEVENTMSG,
                                init_pid.as_raw(),
                                0,
                                &mut _ev as *mut _ as *mut libc::c_void,
                            );
                        }
                    }
                    let cont_sig = if event == 0 { Some(sig) } else { None };
                    let csig = cont_sig.map_or(0, |s| s as i32);
                    unsafe { libc::ptrace(libc::PTRACE_CONT, init_pid.as_raw(), 0, csig); }
                    continue;
                }

                if !process.contains(&tpid) {
                    process.insert(tpid);
                    unsafe {
                        libc::ptrace(
                            libc::PTRACE_SETOPTIONS,
                            tpid.as_raw(),
                            0,
                            PTRACE_O_TRACEEXEC as i32,
                        );
                    }
                    unsafe { libc::ptrace(libc::PTRACE_CONT, tpid.as_raw(), 0, 0); }
                    continue;
                }

                if event == 0x4
                    && sig == Signal::SIGTRAP
                    && !STOP_TRACE_ZYGOTE.load(Ordering::Acquire)
                {
                    if let Ok(exe) = std::fs::read_link(format!("/proc/{}/exe", tpid)) {
                        let path = exe.to_string_lossy();
                        if is_zygote(&path) {
                            let libpath = format!("{}/zygisk.so", get_magisk_tmp());
                            let pid_str = tpid.as_raw().to_string();

                            unsafe { libc::kill(tpid.as_raw(), libc::SIGSTOP); }
                            unsafe { libc::ptrace(libc::PTRACE_CONT, tpid.as_raw(), 0, 0); }
                            let stopped = matches!(
                                waitpid(Some(tpid), Some(WaitPidFlag::__WALL)),
                                Ok(WaitStatus::Stopped(_, _))
                            );
                            if stopped {
                                unsafe {
                                    libc::ptrace(
                                        libc::PTRACE_DETACH,
                                        tpid.as_raw(),
                                        0,
                                        libc::SIGSTOP as i32,
                                    );
                                }
                            } else {
                                unsafe {
                                    libc::ptrace(libc::PTRACE_DETACH, tpid.as_raw(), 0, 0);
                                }
                            }
                            process.remove(&tpid);

                            let status = std::process::Command::new("/proc/self/exe")
                                .arg("zygisk")
                                .arg("trace_zygote")
                                .arg(&pid_str)
                                .arg(&libpath)
                                .stdin(std::process::Stdio::null())
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();

                            match status {
                                Ok(s) if s.success() => {
                                    info!("zygisk: injected into zygote pid={} path={}", tpid, path);
                                }
                                _ => {
                                    error!("zygisk: tracer spawn failed for pid={}", tpid);
                                    unsafe { libc::kill(tpid.as_raw(), libc::SIGKILL); }
                                }
                            }
                            continue;
                        }
                    }
                }

                process.remove(&tpid);
                unsafe { libc::ptrace(libc::PTRACE_DETACH, tpid.as_raw(), 0, 0); }
            }
            Ok(WaitStatus::Stopped(tpid, sig)) => {
                unsafe {
                    libc::ptrace(libc::PTRACE_CONT, tpid.as_raw(), 0, sig as i32);
                }
            }
            Ok(WaitStatus::Exited(tpid, _)) | Ok(WaitStatus::Signaled(tpid, _, _)) => {
                process.remove(&tpid);
                if attached_via_fallback && process.is_empty() {
                    info!("zygisk: fallback: attached zygote exited, re-entering polling");
                    is_fallback = true;
                }
            }
            Err(_) => {
                error!("zygisk: init_monitor waitpid error, stopping");
                break;
            }
            _ => {}
        }
    }
    info!("zygisk: init_monitor exited");
}
