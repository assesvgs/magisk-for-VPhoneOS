use crate::ffi::get_magisk_tmp;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use base::{error, info, libc};


// Android NDK libc 缺少部分 ptrace 常量，手动补充
const PTRACE_SEIZE: libc::c_int = 0x1401;
const PTRACE_O_TRACEFORK: libc::c_int = 0x00000002;
const PTRACE_O_TRACEEXEC: libc::c_int = 0x00000008;

pub static STOP_TRACE_ZYGOTE: AtomicBool = AtomicBool::new(false);

pub fn start_zygisk() {
    std::thread::spawn(|| {
        init_monitor();
    });
}

fn is_zygote(exe_path: &str) -> bool {
    exe_path.contains("app_process")
}

pub fn init_monitor() {
    let init_pid = Pid::from_raw(1);

    let opts = PTRACE_O_TRACEFORK as i32;
    if unsafe { libc::ptrace(PTRACE_SEIZE, init_pid.as_raw(), 0, opts) } == -1 {
        error!("zygisk: PTRACE_SEIZE init(1) failed, disabling zygisk");
        crate::daemon::MAGISKD
            .get()
            .map(|d| d.zygisk.lock().ptrace_seize_failed = true);
        return;
    }
    info!("zygisk: init_monitor started, tracing PID 1");

    let mut process: HashSet<Pid> = HashSet::new();

    loop {
        if STOP_TRACE_ZYGOTE.load(Ordering::Acquire) {
            info!("zygisk: init_monitor stopping (STOP_TRACE_ZYGOTE set)");
            unsafe { libc::ptrace(libc::PTRACE_DETACH, init_pid.as_raw(), 0, 0); }
            break;
        }
        match waitpid(None, Some(WaitPidFlag::__WALL)) {
            Ok(WaitStatus::PtraceEvent(tpid, sig, event)) => {
                if tpid == init_pid {
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

                            // SIGSTOP protocol: pause zygote, keep it stopped for tracer
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
                                    info!(
                                        "zygisk: injected into zygote pid={} path={}",
                                        tpid, path
                                    );
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
