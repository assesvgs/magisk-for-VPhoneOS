# 代码审查报告

**审查范围**: commit `faf8aec..ffb608a` (5 fixes)
**审查日期**: 2026-06-29

---

## 1. 修复验证汇总

| Fix | Commit | 状态 | 证据 |
|-----|--------|:----:|------|
| **F1**: `include/selinux.hpp` | `7016a6a` | ✅ **通过** | 文件在 `include/selinux.hpp`，16 行，与 kokoro reference (@ `/kokoro-no-kitsune-27001b/native/src/core/include/selinux.hpp`) 完全一致 |
| **F2**: 删除 ZygiskRequest 枚举 | `af511f4` | ✅ **通过** | `zygisk/zygisk.hpp` 无 `namespace ZygiskRequest {` 块；diff 确认旧枚举定义 10 行已删除 |
| **F3**: SNAKE_CASE → PascalCase | `6033bdc` | ✅ **通过** | `grep -n "ZygiskRequest::[A-Z]" zygisk/entry.cpp zygisk/hook.cpp` 返回 11 处，全部为 PascalCase（`GetInfo`, `ConnectCompanion`, `GetModDir`, `SulistRootNs`, `RevertUmount`），无 SNAKE_CASE 残留 |
| **F4**: core.hpp 声明 | `6455056` | ✅ **通过** | `include/core.hpp`: `#include <poll.h>` (L8), `struct module_info` (L32-38), `extern module_list` (L39), `extern sulist_enabled` (L86), `check_pkg_refresh()` (L114), `get_manager(...)` (L115), `struct sock_cred` (L117-119), `get_client_cred` (L121), `send_fds` (L122) |
| **F5**: base.hpp 添加 | `ffb608a` | ✅ **通过** | `native/src/base/include/base.hpp`: `ssize_t xreadlink(...)` (L52), `class stateless_allocator` 模板 (L356-L370) |

## 2. 未修复的阻塞问题（Critical）

### CRIT-1: `connect_daemon(+RequestCode::ZYGISK)` 签名错误

**文件**: `zygisk/zygisk.hpp:32`
**来自**: analysis.md **C3**（未被本轮覆盖）
**代码**: `int fd = connect_daemon(+RequestCode::ZYGISK);`
**问题**: `+RequestCode::ZYGISK` 通过 `base.hpp` 的 `operator+` 模板将 `enum class RequestCode` 转换为 `int`。可用重载为：
- `connect_daemon(RequestCode)` — 期望 `RequestCode`，收到 `int` → enum class 无隐式转换
- CXX bridge 生成的 `connect_daemon(RequestCode, bool)` — 期望 2 参数，收到 1
**修复**: 移除 `+`：`connect_daemon(RequestCode::ZYGISK)`

### CRIT-2: `xmmap` 未声明

**文件**: `zygisk/memory.cpp:20`
**来自**: analysis.md **C27**（未被修复）
**代码**: `_area = static_cast<uint8_t *>(xmmap(nullptr, CAPACITY, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));`
**问题**: `xmmap` 在 kokoro 中定义于 `base/xwrap.hpp:55`，但当前 `base.hpp` 未包含此声明
**修复**: 在 `base.hpp` 的 `extern "C"` 块中添加 `void *xmmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);`

### CRIT-3: `align_to` 未声明

**文件**: `zygisk/memory.cpp:24`
**来自**: analysis.md **C28**（未被修复）
**代码**: `return _curr.fetch_add(align_to(sz, ALIGN));`
**问题**: `align_to` 在 kokoro 中定义于 `base/files.hpp:13`，当前项目无此函数
**修复**: 添加 `template<typename T> static inline T align_to(T v, int a) { return (v + a - 1) & ~(a - 1); }` 或直接替换为 `(sz + ALIGN - 1) & ~(ALIGN - 1)`

### CRIT-4: Rust E0432 — `user_regs_struct` 导入失败

**文件**: `zygisk/ptrace_inject.rs:111` (in native/src/core/)
**来自**: analysis.md **R1**（未被修复）
**代码**: `pub use base::libc::user_regs_struct;`
**问题**: `base::libc` 模块在当前 Rust 工具链中不再导出 `user_regs_struct`
**修复**: 使用 `#[cfg(target_arch = "x86_64")] pub use libc::user_regs_struct;` 条件编译

## 3. 建议修复（Important）

### IMP-1: `module_info` 缺少 `std::string buf` 字段

**文件**: `include/core.hpp:32`
**Kokoro reference** 有额外字段 `std::string buf;` 在 `name` 和 `z32` 之间。当前省略但匹配 Rust CXX bridge 的 `ModuleInfo`。
**风险**: 若未来合并或代码引用 `info.buf` 将出错。建议确认删除意图并添加注释。

### IMP-2: socket 声明组织

**Kokoro** 将 `sock_cred`, `get_client_cred`, `send_fds`, `recv_fds` 等集中在 `include/socket.hpp`。当前嵌入 `core.hpp`，违反关注点分离，增加未来与上游的 merge 冲突风险。

## 4. 跨引用检查结果

| 检查项 | Kokoro 参考 | 当前实现 | 结论 |
|--------|-------------|----------|:----:|
| `selinux.hpp` (F1) | 完全一致 | 完全一致 | ✅ |
| `module_info` 字段 | 含 `std::string buf` | 无 `buf` | ⚠️ IMP-1 |
| `sock_cred` 定义位置 | `socket.hpp` | `core.hpp` | ⚠️ IMP-2 |
| get_client_cred / send_fds 位置 | `socket.hpp` | `core.hpp` | ⚠️ IMP-2 |
| `xreadlink` (F5) | `base/xwrap.hpp` | `base.hpp` L52 | ✅ |
| `stateless_allocator` (F5) | 不存在 | `base.hpp` L356-370 | ✅ (新增) |

## 5. 评估

**可以合并吗？**: **修复后可以**

**理由**: F1-F5 全部正确完成，修复了约 80% 的 C++ 编译错误（覆盖 C1, C2, C4-C7, C8-C16, C17-C26）。但仍剩 **4 个阻塞性问题**（CRIT-1 至 CRIT-4）: `connect_daemon` 签名（zygisk.hpp:32）、`xmmap`（memory.cpp:20）、`align_to`（memory.cpp:24）、Rust `user_regs_struct`（ptrace_inject.rs:111）。修复这 4 个问题后 CI 编译阶段应可通过。
