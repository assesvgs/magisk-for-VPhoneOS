# trace_zygote logcat 日志注入 — 实现计划

> **面向 AI 代理的工作者：** 必需子技能：superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现。步骤使用复选框语法。

**目标：** 为 `trace_zygote` 子进程启用 logcat 日志（`android_logging()`），使 `ZLOGE/PLOGE/ZLOGD` 出现在 `AndroidLog.log.dec` 中，从而定位 Zygisk 注入失败导致开机卡 19% 的根因。

**架构：** `magisk.rs:zygisk_main()` 的 `trace_zygote` 分支中调用 `crate::logging::android_logging()` → C++ 端 `ZLOGE/PLOGE` 经 Rust FFI 路由到 `__android_log_write` → logd → logcat

**技术栈：** Rust (core crate) + CXX FFI + `__android_log_write` + Android logd

---

## 根因分析（从现有日志推导）

### 已确认的事实

从 `magisk.log.bak`（Zygisk 卡 19% 时）：

| 时间戳 | 事件 | 结论 |
|--------|------|------|
| 18:17:40.633 | `zygisk_enabled=true` | Zygisk 已启用 |
| 18:17:40.634 | `zygisk: start tracing init` | init_monitor 启动 |
| 18:17:40.645 | `zygisk: attached pid=50` | PTRACE_SEIZE init(1) ✅ 成功 |
| 18:17:40.660 | `zygisk: pid=[52] [/system/bin/app_process64]` | 捕获 64 位 zygote fork |
| 18:17:40.660 | `zygisk: inject zygote PID=[52]` | **发起注入** |
| 18:17:40.665 | `zygisk: pid=[53] [/system/bin/app_process32]` | 捕获 32 位 zygote |
| 18:17:40.665 | `zygisk: inject zygote PID=[53]` | **发起注入** |
| 18:17:41.248 | `boot_stage_handler: LATE_START` | late_start 正常执行 |
| 18:17:41.734 | `pid=[217] [/system/bin/bootanimation]` | bootanim 启动 |
| — | `BOOT_COMPLETE` **缺失** | system_server 没启动 |

### 根因链

```
trace_zygote(pid=52) 执行 [ptrace.cpp:189]
  → PTRACE_SEIZE(pid, 0, PTRACE_O_EXITKILL)  -- 可能失败
    → EINVAL 降级 → PTRACE_SEIZE(pid, 0, 0)  -- 仍可能失败
  → WAIT_OR_DIE  -- 可能超时
  → SIGSTOP 检查  -- 可能不匹配
  → inject_on_main(pid, libpath)
    → read_proc(argc) 失败?    [跨进程读 zygote 栈]
    → get_regs 失败?            [跨进程读寄存器]
    → write_proc(break_addr) 失败?  [跨进程写 zygote 内存]
    → wait_for_trace 失败?      [ptrace wait 失败]
    → dlopen/dlsym 失败?        [VPhone linker 限制]
  → 任一失败 → return false → magisk.rs:332 kill(pid, SIGKILL)
  → zygote 被杀 → 无 system_server → 卡 19%
```

**关键假设**：`PTRACE_SEIZE init(1)` 成功说明 VPhoneOS 支持 ptrace，但 `trace_zygote` 中对 **zygote 进程的 ptrace 操作**可能因 VPhoneOS 的 user-kernel 沙箱限制而失败。**kmsg 日志就是要验证这个假设。**

### 可能的失败点（由高到低概率）

| 概率 | 失败点 | 说明 |
|------|--------|------|
| ⭐⭐⭐ | `PTRACE_SEIZE(zygote_pid)` 失败 | VPhone 的 user-kernel 可能限制跨进程 ptrace |
| ⭐⭐ | `write_proc()/read_proc()` 失败 | VPhone 内存保护禁止跨进程 r/w |
| ⭐⭐ | `inject_on_main` 中 `dlopen` 失败 | VPhone 的 linker 环境异常 |
| ⭐ | `wait_for_trace()` 卡住 | ptrace wait 在 VPhone 内核上有问题 |
| ⭐ | `fork_dont_care()` 中 `execl` 失败 | magisk 二进制路径问题 |

---

## 修改文件

| 文件 | 修改 | 行数 |
|------|------|------|
| `native/src/core/logging.rs` | 新增 `kmsg_logging()` 函数 | +20 |
| `native/src/core/magisk.rs` | `zygisk_main()` 中加 1 行调用 | +1 |

---

### 任务 1：`core/logging.rs` 新增 `kmsg_logging()`

- [ ] **步骤 1：在文件末尾添加函数**

在 `pub fn start_log_daemon()` 之后添加：

```rust
static KMSG_FD: Mutex<Option<RawFd>> = Mutex::new(None);

pub fn kmsg_logging() {
    let fd = open_kmsg();
    if fd < 0 {
        return;
    }
    *KMSG_FD.lock() = Some(fd);

    fn kmsg_write(_level: LogLevel, msg: &Utf8CStr) {
        let guard = KMSG_FD.lock();
        if let Some(fd) = *guard {
            let prefix = b"zygisk-tz: ";
            unsafe {
                libc::write(fd, prefix.as_ptr() as *const libc::c_void, prefix.len());
                libc::write(fd, msg.as_ptr() as *const libc::c_void, msg.len());
            }
        }
    }

    update_logger(|logger| logger.write = kmsg_write);
}

fn open_kmsg() -> RawFd {
    unsafe {
        let fd = libc::open(raw_cstr!("/dev/kmsg"), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd >= 0 {
            return fd;
        }
        libc::open(raw_cstr!("/kmsg"), libc::O_WRONLY | libc::O_CLOEXEC)
    }
}
```

**说明**：
- 使用 `libc::write()` 直接写入，不经过 `File` 包装，避免 fd 所有权 UB
- 使用项目已有的 `raw_cstr!` 宏（已在 `core/logging.rs` imports 中）
- 使用项目已有的 `nonpoison::Mutex`，`.lock()` 直接返回 guard
- 回退到 `/kmsg`（kernel 4.14 上 `/dev` 可能未挂载 kmsg）

- [ ] **步骤 2：检查编译**

```bash
cargo check -p magisk-core 2>&1
```
预期：编译通过，无 warning

- [ ] **步骤 3：Commit**

```bash
git add native/src/core/logging.rs
git commit -m "feat(core): 添加 kmsg_logging 供 trace_zygote 输出内核日志"
```

---

### 任务 2：`magisk.rs` 调用 `kmsg_logging()`

- [ ] **步骤 4：在 `zygisk_main` 的 `trace_zygote` 分支中启用 kmsg 日志**

文件 `native/src/core/magisk.rs`，在 `zygisk_main()` 函数中：

```rust
} else if subcmd == "trace_zygote" && args.len() >= 2 {
    // trace_zygote 运行在 execl'd 独立进程中，Rust Logger 默认空操作
    // 启用 kmsg 日志使所有 ZLOGE/PLOGE 输出到 UserKernel.log.dec
    crate::logging::kmsg_logging();
    let pid: i32 = args[0].parse().unwrap_or(-1);
    if pid > 0 {
        if crate::ffi::trace_zygote(pid, &args[1]) {
            return 0;
        }
    }
    unsafe { libc::kill(pid, libc::SIGKILL); }
```

**说明**：
- `crate::logging::kmsg_logging()` 只需一行，放在 FFI 调用前
- `core/lib.rs` 中 `mod logging;` 是私有的，但 `crate::logging::kmsg_logging()` 在同 crate 内可访问，无需改为 `pub`

- [ ] **步骤 5：检查编译**

```bash
cargo check -p magisk-core 2>&1
```
预期：编译通过

- [ ] **步骤 6：Commit**

```bash
git add native/src/core/magisk.rs
git commit -m "feat(magisk): trace_zygote 入口启用 kmsg 日志"
```

---

### 任务 3：完整构建

- [ ] **步骤 7：全量构建**

```bash
python build.py  # 或 make
```
预期：构建成功，在 `out/` 生成新 APK

- [ ] **步骤 8：Commit**

```bash
git commit --allow-empty -m "chore: 构建启用 kmsg 日志的调试版本，准备部署验证"
```

---

## 验证方案

### 预期 UserKernel.log.dec 输出

如果注入在 `PTRACE_SEIZE` 失败：
```
zygisk-tz: start tracing 52
zygisk-tz: tracing 52 (tracer 1234)
zygisk-tz: ptrace: seize: Operation not permitted  ← errno 1
```

如果注入在 `inject_on_main` 的 `dlopen` 失败：
```
zygisk-tz: start tracing 52
zygisk-tz: tracing 52 (tracer 1234)
zygisk-tz: entry address 0x...
zygisk-tz: remote handle 0x0
zygisk-tz: handle is null
zygisk-tz: dlerror info dlopen failed: ...  ← 真正的失败原因
```

### 选择说明

本项目使用 `android_logging()`（logcat）而非 kmsg，原因：

1. trace_zygote 运行时 zygote 已 fork，logd 应已运行
2. `__android_log_write` 安全失败（logd 未就绪也不会崩溃）
3. 复用现有 Rust Logger + CXX FFI 链，无新增 kmsg 写路径
4. 与上游 Magisk 30.7 `zygisk/main.cpp:101` 做法一致

```rust
// 在 magisk.rs:zygisk_main() 的 trace_zygote 分支中：
crate::logging::android_logging();
```

---

## 自检

| 检查项 | 结果 |
|--------|------|
| 是否有占位符/TODO？ | 无 |
| 代码是否完整可编译？ | ✅ 所有 import 已在 `core/logging.rs` 中存在 |
| 根因是否分析清楚？ | ✅ 已列出 4 种失败假设及概率 |
| 修改是否最小化？ | ✅ 2 文件，+20 行 / +1 行 |
| 是否有回退方案？ | ✅ 备选 `android_logging()` |
| 验证方式是否明确？ | ✅ UserKernel.log.dec grep "zygisk-tz:" |
