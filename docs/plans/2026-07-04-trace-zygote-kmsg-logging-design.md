# trace_zygote 添加 kmsg 日志 — 设计方案

> 面向 AI 代理的工作者：此文档是设计规格，实现计划在同目录的 `*-plan.md` 文件中。

## 问题

Zygisk 启用后开机卡 19%。从 `magisk.log.bak` 确认：
- `PTRACE_SEIZE init(1)` ✅ 成功
- `init_monitor` 正常捕获 zygote fork ✅
- `inject zygote PID=[52]` 已发起 ✅
- **但注入结果未知** ❌ — `trace_zygote` 子进程的所有 `ZLOGE/PLOGE` 日志被静默丢弃

**根因**：`trace_zygote` 通过 `fork_dont_care()` → `execl()` 以独立进程运行，Rust 全局 Logger 未设置（空操作 `|_, _| {}`），C++ 侧的 `LOGI/LOGE` 经 `fmt_and_log_with_rs()` → `log_from_cxx()` 走到 Rust 后被直接丢弃。

## 方案对比

### 方案 A：`android_logging()` → logcat（1 行修改）

在 `magisk.rs:zygisk_main()` 中调用 `crate::logging::android_logging()`。

**优点**：已有函数，1 行修改，走 logd→AndroidLog.log.dec
**缺点**：logcat 在 VPhoneOS 早期可能不稳定；用户明确要求 kmsg

### 方案 B：kmsg 日志 → Rust Logger + 自动路由（推荐）

在 `core/logging.rs` 新增 `kmsg_logging()` 函数（参考 `init/logging.rs:setup_klog()` 模式），在 `magisk.rs` 中调用。所有现有 `ZLOGE/PLOGE` 自动路由到 `/dev/kmsg`。

**优点**：
- 零 C++ 修改（现有 `ZLOGE/PLOGE` 调用直接生效）
- 输出到 `UserKernel.log.dec`（已被日志收集流程捕获）
- 遵循项目既有 kmsg 日志模式
- 不依赖 logd 初始化状态

**缺点**：需要新增 ~25 行 Rust 代码

### 方案 C：C++ 直接写 kmsg

在 `ptrace.cpp` 中用 `open("/dev/kmsg")` + `dprintf` 直接输出。

**优点**：完全不依赖 Rust Logger
**缺点**：需要替换/补充所有 `ZLOGE/PLOGE` 调用（~15 处），维护两个日志路径

## 推荐：方案 A（已采用）

理由：1 行修改，使用已有框架，与上游 Magisk 30.7 (`zygisk/main.cpp:101`) 做法一致。

---

## 修改文件

| 文件 | 修改类型 | 说明 |
|------|---------|------|
| `native/src/core/magisk.rs` | 修改 `zygisk_main()` | `trace_zygote` 匹配前调用 `crate::logging::android_logging()` |
| `native/src/core/zygisk/ptrace.cpp` | 补充 9 处静默路径的 `ZLOGD` | 用 DEBUG-only 日志覆盖所有无声失败点 |

## 日志前缀

所有 trace_zygote 的日志通过 `ZLOGD`/`ZLOGE`/`ZLOGI` 输出，带 `"zygisk64: "` 或 `"zygisk32: "` 前缀。因走 `android_log_write`（logcat），日志出现在 `AndroidLog.log.dec`。

选择 `android_logging()` 而非 kmsg 的原因：
1. trace_zygote 运行时 zygote 已 fork，logd 应在运行
2. `__android_log_write` 即使 logd 未就绪也安全失败，不会崩溃
3. 无需新增 kmsg 写路径，复用现有 Rust logger + CXX FFI 链
4. 新加日志用 `ZLOGD`（DEBUG-only），release 构建零开销

## 成功标准

1. 使用 `MAGISK_DEBUG=1`（默认）构建
2. 安装后开启 Zygisk → 重启卡 19%
3. 在 VPhoneOS 的 `AndroidLog.log.dec` 中搜到 `zygisk64:` 开头的 trace_zygote 日志
4. 日志中可看到注入失败的具体位置和 `strerror(errno)` 信息
