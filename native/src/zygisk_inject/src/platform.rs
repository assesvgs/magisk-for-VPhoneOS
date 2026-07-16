//! 跨平台兼容性适配。
//!
//! libc crate 在不同目标平台（linux/android/macos/windows）的 API 和类型有差异。
//! 此模块统一封装这些差异，调用方无需关心平台细节。

use core::ffi::c_void;

/// 检查 stat 结构是否代表常规文件。
/// st_mode 和 S_IFMT/S_IFREG 的类型在不同平台不同
///（例如 macOS 上 st_mode=u32 但 S_IFMT=u16），统一 cast 避免类型错误。
pub fn is_regular_file(st: &libc::stat) -> bool {
    (st.st_mode as u32 & libc::S_IFMT as u32) == libc::S_IFREG as u32
}

/// 跨平台一致的 CMSG_LEN 包装。
/// libc crate 的 CMSG_LEN 是 unsafe fn，且在不同目标平台返回类型不同
///（usize 或 u32），统一转换为 usize 避免类型错误。
pub unsafe fn cmsg_len(len: usize) -> usize {
    libc::CMSG_LEN(len as u32) as usize
}

/// 跨平台一致的 CMSG_SPACE 包装，与 cmsg_len 对称。
pub unsafe fn cmsg_space(len: usize) -> usize {
    libc::CMSG_SPACE(len as u32) as usize
}
