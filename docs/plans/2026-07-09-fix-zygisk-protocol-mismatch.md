# Zygisk 黑屏问题修复计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

---

## 第一部分：已确认的缺陷修复（代码级验证通过）

本部分的 3 处错误均为代码审查确认的 Rust-C++ 协议不匹配，`socket.rs` 和 `daemon.rs` 中的白纸黑字，**无需推测**。kokoro（纯 C++ 实现）无此问题。

### 已确认的 3 处缺陷

| # | 问题 | Rust 侧 | C++ 侧 | 影响 |
|---|------|---------|--------|------|
| 1 | `is_64bit` 类型宽度 | `bool::decode` → 1 字节 | `write_int` → 4 字节 | 每次调用残留 3 字节，错位所有后续读取 |
| 2 | `failed_ids` 元素宽度 | `Vec<i32>` → 4 字节每元素 | `unsigned long` → 8 字节每元素 | `connectCompanion 的 `module_id` 和 system_server 模块状态解析完全错位 |
| 3 | fd 缺省值 -1 | `fd < 0 ? -1 : fd` | 期待合法 fd | `sendmsg(-1)` → `EBADF` → `send_fds` 静默失败 |

### 修改文件

| 文件 | 改动 | 原因 |
|------|------|------|
| `native/src/core/zygisk/daemon.rs` | 5 处修改 | `bool`→`i32`（2 处）、`is_64_bit` 使用方式调整（2 处）、`Vec<i32>`→`Vec<i64>`（1 处）、`-1`→`1`（1 处） |
| `native/src/core/socket.rs` | 1 处添加 | `impl_pod_encodable!` 加入 `i64` |

**原则：** Rust 侧适配 C++ 侧。不动 C++ client 一行代码。

---

### 任务 1：修复 `is_64bit` 类型宽度错位（4 字节→1 字节）

**代码证据：** `socket.rs:48-53` — `bool::decode` 读 `u8`（1 字节）。`socket.rs:36` — `impl_pod_encodable! { u8 u32 i32 usize }`。C++ 侧 `write_int(fd, val)` 写 4 字节。

**文件：** `native/src/core/zygisk/daemon.rs`

**影响的两处位置：**
- `connect_zygiskd:27`：`client.read_decodable::<bool>()`
- `get_process_info:164`：`client.read_decodable::<bool>()`

- [ ] **步骤 1：将 connect_zygiskd:27 的 bool 改为 i32**

```rust
// before:
let is_64_bit: bool = client.read_decodable().ok()?;
// after:
let is_64_bit: i32 = client.read_decodable().ok()?;
```

- [ ] **步骤 2：修复 connect_zygiskd 中所有 is_64_bit 引用**

```rust
let slot = if is_64_bit != 0 { &mut self.sockets.1 } else { &mut self.sockets.0 };
exec_zygiskd(is_64_bit != 0, remote);
if let Some(module_fds) = daemon.get_module_fds(is_64_bit != 0) {
```

- [ ] **步骤 3：将 get_process_info:164 的 bool 改为 i32**

```rust
// before:
let is_64_bit: bool = client.read_decodable().ok()?;
// after:
let is_64_bit: i32 = client.read_decodable().ok()?;
```

- [ ] **步骤 4：修复 get_process_info 中 is_64_bit 的使用**

```rust
if let Some(fds) = self.get_module_fds(is_64_bit != 0) {
```

- [ ] **步骤 5：编译验证**

```bash
python build.py
```
预期：编译通过，无类型错误。

---

### 任务 2：修复 `failed_ids` 元素宽度错位（8 字节→4 字节）

**代码证据：** `socket.rs:36` — `impl_pod_encodable!` 当前仅有 `{ u8 u32 i32 usize }`，无 `i64`。`hook.cpp:667` — C++ `xwrite(fd, &l, sizeof(l))` 中 `l` 为 `unsigned long`（8 字节）。`daemon.rs:188` — Rust `Vec<i32>::decode` 读 4 字节每元素。

**文件：** `native/src/core/socket.rs`、`native/src/core/zygisk/daemon.rs`

- [ ] **步骤 1：在 socket.rs 的 impl_pod_encodable! 中添加 i64**

```rust
// native/src/core/socket.rs:36
// before:
impl_pod_encodable! { u8 u32 i32 usize }
// after:
impl_pod_encodable! { u8 u32 i32 i64 usize }
```

- [ ] **步骤 2：将 daemon.rs:188 的 Vec\<i32\> 改为 Vec\<i64\>**

```rust
// before:
let _failed_ids: Vec<i32> = client.read_decodable().ok()?;
// after:
let _failed_ids: Vec<i64> = client.read_decodable().ok()?;
```

- [ ] **步骤 3：编译验证**

```bash
python build.py
```
预期：编译通过。

---

### 任务 3：修复 `get_module_fds` 返回 `-1`

**代码证据：** `daemon.rs:198`：`if fd < 0 { -1 } else { fd }`。`sendmsg` 对 fd=-1 返回 `EBADF` → `send_fds` 中的 `?` 提前返回 None。

**文件：** `native/src/core/zygisk/daemon.rs`

- [ ] **步骤 1：将 -1 替换为 1**

```rust
// before:
if fd < 0 { -1 } else { fd }
// after:
if fd < 0 { 1 } else { fd }
```

- [ ] **步骤 2：编译验证**

```bash
python build.py
```
预期：编译通过。

---

### 任务 4：集成验证

- [ ] **步骤 1：完整构建**

```bash
python build.py
```
预期：所有 ABI 的 `magisk`、`magisk32`、`magisk64` 编译成功。

- [ ] **步骤 2：确认产物**

```bash
file out/*/magisk32 | head -3
```

- [ ] **步骤 3：提交**

```bash
git add -A
git commit -m "fix(zygisk): 修复 Rust-C++ 协议错位——is_64bit 类型宽度/failed_ids 元素宽度/fd 缺省值"
```

---

## 第二部分：未确认的潜在因素（需进一步验证）

以下问题确认存在代码级差异，但**不确定是否为首开机黑屏的直接原因**。待第一部分修复完成后，用 debug 版在 VPhoneOS 上验证开机情况，如仍有问题再从此章节排查。

### P1：denylist 标志位不匹配（bit 30 vs bit 28）

| 侧 | 枚举值 | 文件 |
|----|--------|------|
| Rust `DenyListEnforced` | `0x40000000`（bit 30） | `lib.rs:86` |
| Rust `UNMOUNT_MASK` | `ProcessOnDenyList \| DenyListEnforced`（bit 1 \| 30） | `daemon.rs:11-12` |
| C++ `MAGISKHIDE_ENABLED` | `1u << 28`（bit 28） | `module.hpp:128` |
| C++ `UNMOUNT_MASK` | `PROCESS_ON_DENYLIST \| MAGISKHIDE_ENABLED`（bit 1 \| 28） | `module.hpp:132` |

**影响：** Rust 写回给 C++ 的 flags 中 bit 30 置位，但 `hook.cpp:620` 检查 bit 28，`DO_REVERT_UNMOUNT` 和 `DO_FUTILE_HIDE` 永远不会执行。**但 denylist 功能失效不会导致崩溃。** 验证首开机修复后，再决定是否修复。

### P2：32 位 JNI hook 与 VPhoneOS ART 兼容性

**事实：**
- 53ee70b4 32 位注入失败（step=52），系统正常开机
- 1b3f11a6 32 位注入成功，Settings 的 RenderThread SIGABRT
- kokoro 同套 hook.cpp 工作正常（但可能测试时 VPhoneOS 版本不同）

**推测：** VPhoneOS Android 10 的 ART 对 `env->functions` 表替换有检测，32 位 zygote 加载 zygisk32 后，32 位 system 进程的 JNI 调用触发 ART 的异常检测。

**验证方式：** 在 VPhoneOS 上用 debug 版本抓 logcat，搜索 `art::Runtime::Abort` 或 `FatalError` 相关的 stack trace。
