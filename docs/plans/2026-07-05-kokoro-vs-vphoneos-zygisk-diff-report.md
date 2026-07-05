# kokoro-no-kitsune-27001b vs magisk-for-VPhoneOS Zygisk 差异报告

> 生成日期：2026-07-05，基于 commit `0197fa0` vs kokoro `27001b`
> 范围：Zygisk 相关 C++ 代码的逐文件对比

---

## 1. 架构概览

| 层面 | kokoro | 本项目 |
|------|--------|--------|
| 守护进程 | 纯 C++（`daemon.cpp`） | C++ + Rust 混合（`daemon.rs` + 部分 main.cpp） |
| 注入监控 | `proc_monitor.cpp` | `init_monitor.cpp`（重写版） |
| 跟踪器二进制 | `magisk64` / `magisk32` | `magisk`（统一单二进制） |
| zygote 匹配 | 仅 `app_process32/64` | `app_process`、`app_process32/64` |
| 错误处理 | `exit(1)` → 静默终止 tracer | `return false` → `SIGKILL` zygote |
| 部分写入检查 | 宽松（传递部分长度） | 严格（转为 -1 视为失败） |
| 崩溃检测 | C++ `reset_zygisk()` | Rust `ZygiskState::reset()`（不同实现） |

---

## 2. init_monitor.cpp（原 proc_monitor.cpp）差异

| 差异点 | kokoro | 本项目 | 影响评估 |
|--------|--------|--------|---------|
| sulist 支持 | 完整（`unmount_zygote`、`wait_unmount`） | 已移除 | 无直接影响 |
| init 回退 | 无（`goto abandon` 退出线程） | 轮询 `/proc` 回退 | ✅ 提升 |
| zygote 匹配 | 仅 `32/64` | 也匹配 `app_process` | **可能引入问题** |
| tracer 路径 | `magisk64` / `magisk32` | `magisk`（单二进制） | 架构兼容性 |
| fork 方式 | `fork_dont_care()` 不等待 | `xfork()` + waitpid 轮询 5s | ✅ 改进（本 commit） |
| ECHILD 处理 | 无 | `nanosleep(INT_MAX)` | ✅ 改进 |

---

## 3. ptrace.cpp + ptrace_utils.cpp 差异（关键）

### 3.1 `write_proc` / `read_proc` 部分写入处理

这是**最关键的差异**之一。

```
kokoro:   process_vm_writev 返回 l（可能部分）→ return l
          调用方：if (!write_proc(...)) → 非零视为成功

本项目：  process_vm_writev 返回 l → 如果 l != len → l = -1
          调用方：if (write_proc(...) <= 0) → -1 视为失败
```

**影响**：如果 VPhoneOS 内核 4.14.42-super 上 `process_vm_writev` 返回了部分长度（标准 Linux 上罕见但可能），kokoro 宽松通过，本项目直接让注入失败。

### 3.2 `wait_for_trace` 函数签名

```
kokoro:   void wait_for_trace(...) → 失败时 exit(1)，tracer 进程自杀
本项目：   bool wait_for_trace(...) → 失败时 return false
          调用方（WAIT_OR_DIE）：return false → trace_zygote return false
          → magisk.rs: kill(pid, SIGKILL)
```

**影响**：VPhoneOS 上 ptrace wait 如果失败（比如 VPhone 内核不兼容），kokoro 静默自杀、zygote 不受影响；本项目直接杀 zygote。

### 3.3 `PTRACE_O_EXITKILL` 降级

```
kokoro:   PTRACE_SEIZE(pid, 0, O_EXITKILL) 失败 → return false
本项目：   O_EXITKILL 失败 → 判断 EINVAL → 降级重试不带 O_EXITKILL
```

**影响**：本项目比 kokoro 更兼容（内核 < 5.3 也可工作）。✅ 改进。

### 3.4 `inject_on_main` 安全检查

| 检查项 | kokoro | 本项目 |
|--------|--------|--------|
| argc 读取 | 无错误检查 | `if (read_proc(...) != sizeof(argc))` |
| envp 循环 | `while (true)` 无限 | `int envp_limit = 4096` 保护 |
| auxv 循环 | `while (true)` 无限 | `int auxv_limit = 512` 保护 |
| 远程调用失败恢复 | 无 | aarch64/arm 上备份恢复 + DETACH |

**影响**：本项目更安全。✅ 改进。

---

## 4. entry.cpp 差异（zygisk_inject_entry）

### 4.1 `remote_get_info` 协议变更

```cpp
// kokoro:
write_int(fd, uid);
write_string(fd, process);
xxread(fd, flags, sizeof(*flags));

// 本项目：
write_int(fd, uid);
write_string(fd, process);
write_int(fd, sizeof(void*) == 8 ? 1 : 0);  // ← 新增 is_64bit
xxread(fd, flags, sizeof(*flags));
```

**影响**：本项目发送 `is_64bit` 到 Rust 侧。Rust 端也正确读取了该字段。**协议一致**，安全。

### 4.2 `recv_fds` 返回值类型

```cpp
// kokoro:
fds = recv_fds(fd);

// 本项目：
{ rust::Vec<int32_t> tmp = recv_fds(fd); fds.assign(tmp.begin(), tmp.end()); }
```

**影响**：Rust CXX 桥接需要。功能等效。

### 4.3 `reset_zygisk()` — 已移除

kokoro 的 `reset_zygisk()`（`entry.cpp:303-321`）负责：
- 跨 zygote 重启的崩溃计数（`atomic_uint zygote_start_count{1}`）
- 计数 > 3 时停止跟踪
- BOOT_COMPLETE 时重置

**本项目将此逻辑移到了 Rust `ZygiskState::reset()`（`daemon.rs`）。功能等效但实现不同。**

---

## 5. hook.cpp 差异

| 差异 | kokoro | 本项目 |
|------|--------|--------|
| `plt_hook_commit` regex 释放 | **泄漏** | 调用 `regfree()` ✅ 修复 |
| `run_modules_pre` mmap 检查 | 无错误检查 | `if (copy == MAP_FAILED)` ✅ 修复 |
| `run_modules_pre` mremap 检查 | 无错误检查 | 失败时 `munmap` 清理 ✅ 修复 |
| `fork_pre` fd < 0 处理 | `close(fd)`（无害） | `continue` 跳过 ✅ 更安全 |

**影响**：本项目修复了 kokoro 的内存泄漏和 mmap/mremap 错误处理缺失问题。✅ 改进。

---

## 6. main.cpp（zygiskd）差异

```cpp
// kokoro zygisk_main():
} else if (argc == 4 && argv[1] == "trace_zygote"sv) {
    pid_t pid = parse_int(argv[2]);
    if (!trace_zygote(pid, argv[3]))
        kill(pid, SIGKILL);
}

// 本项目 zygisk_main()：没有 trace_zygote 处理！
```

**影响**：本项目的 C++ zygiskd 不再直接处理 `trace_zygote` 命令。该职责移到了 Rust `magisk.rs:zygisk_main()`。**注意：功能链路是完整的**，Rust 端正确代理了 `crate::ffi::trace_zygote()`。

---

## 7. 差异汇总与根因分析

### 最可能导致 3 次开机问题的差异（按可能性排序）

| 排名 | 差异 | 文件 | 机制 | 概率 |
|------|------|------|------|------|
| **1** | 单二进制 `magisk` vs `magisk64/32` | `init_monitor.cpp` | 如果 VPhoneOS 需要 32/64 位分别调用，单二进制在 32 位进程上 ptrace 操作可能失败 | ⭐⭐⭐ |
| **2** | `write_proc` 部分写入转为失败 | `ptrace_utils.cpp` | VPhone 内核的 `process_vm_writev` 如返回部分长度，本项目视为失败，kokoro 宽松通过 | ⭐⭐⭐ |
| **3** | 匹配 bare `app_process` | `init_monitor.cpp` | VPhoneOS 如果有裸 `app_process`（无后缀），本会注入它，而 `trace_zygote` 可能处理不了 | ⭐⭐ |
| **4** | `wait_for_trace` 失败杀 zygote | `ptrace_utils.cpp` | 即使 ptrace 临时失败，kokoro 静默退出，本项目杀 zygote | ⭐⭐ |
| **5** | `reset_zygisk` 移到了 Rust | `entry.cpp` → `daemon.rs` | Rust 实现与 C++ 不一致可能导致崩溃计数行为差异 | ⭐ |

### 建议验证步骤

1. 先跑 `0197fa0` debug 构建，看 `magisk.log` 里 `zygisk: trace_zygote failed` 的行——确认注入确实失败
2. 如确认失败，将 `init_monitor.cpp` 里的 tracer 路径从 `"magisk"` 改为 `"magisk64"`（\__LP64__ 条件编译），测试是否改善
3. 如仍失败，对比 `process_vm_writev` 在 VPhoneOS 内核上的行为——可以考虑将 `ptrace_utils.cpp:78` 的 `l = -1` 改为 kokoro 的宽松模式
