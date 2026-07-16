![](docs/images/kitsune_logo.png)

# Kitsune Mask for VPhoneOS

> 基于 Magisk 30.7，集成 Kitsune Mask 特性，适用于 VPhoneOS 安卓虚拟机环境。

[![Build](https://github.com/assesvgs/magisk-for-VPhoneOS/actions/workflows/build.yml/badge.svg)](https://github.com/assesvgs/magisk-for-VPhoneOS/actions/workflows/build.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Android](https://img.shields.io/badge/Android-6.0%2B-green.svg)](https://developer.android.com/about)

---

## 简介

Kitsune Mask 已停更新版本，且 27001 版本存在 MagiskHide 无法正常卸载 /sbin 的问题。本项目以 Magisk 30.7 为基础，将 Kitsune Mask 的核心功能移植进来，解决了此问题，并增强了反检测能力。

## 主要特性

- [x] **MagiskHide** — 进程级 Magisk 修改隐藏，支持 `--hide` 命令
- [x] **SuList 白名单模式** — 只有列表中的应用才能获取 root 权限
- [x] **/sbin 卸载** — 完整的 /sbin 挂载/卸载功能（`--do-unmount`、`--mount-sbin`）
- [x] **ptrace 进程监控** — 替代 logcat，更可靠、CPU 占用更低
- [x] **SoList 隐藏** — 从 linker 链表中抹掉 Zygisk 模块记录
- [x] **内存重映射隐藏** — 从 /proc/self/maps 中抹掉 Zygisk 模块内存映射
- [x] **JNI hook 内存分配器** — mmap 单调分配器，避免内存碎片痕迹
- [x] **启动时挂载 MagiskSU** — boot_complete 阶段自动挂载

## 快速开始

### 环境要求

- Python 3.8+
- JDK 21
- Android NDK（通过 `./build.py ndk` 自动安装）
- macOS / Linux / Windows

### 构建

```bash
# 克隆仓库
git clone https://github.com/assesvgs/magisk-for-VPhoneOS.git
cd magisk-for-VPhoneOS

# 初始化子模块
git submodule update --init --recursive

# 安装 NDK（首次）
./build.py ndk

# 完整构建
./build.py all
```

### 命令参考

```bash
# MagiskHide
magisk --hide status              # 查看状态
magisk --hide enable              # 启用
magisk --hide disable             # 禁用
magisk --hide add <pkg> [proc]    # 添加到隐藏列表
magisk --hide rm <pkg> [proc]     # 从隐藏列表移除
magisk --hide ls                  # 查看隐藏列表
magisk --hide --do-unmount        # 手动卸载 Magisk 修改
magisk --hide --mount-sbin        # 挂载 /sbin
magisk --hide --setup-sbin        # 设置 /sbin

# SuList（白名单模式）
magisk --hide sulist              # 查看状态
magisk --hide sulist enable       # 启用
magisk --hide sulist disable      # 禁用
```

## 与原版 Magisk 30.7 的差异

| 功能 | 原版 | 本项目 |
|------|------|--------|
| MagiskHide 命令 | `--denylist` | `--hide` |
| SuList 白名单 | ❌ | ✅ |
| /sbin 卸载 | 基础卸载 | 完整卸载（含 early-mount.d） |
| 进程监控 | logcat | ptrace |
| Zygisk 反检测 | 无 | SoList 隐藏 + 内存重映射 + 内存分配器 |
| 包名 | `com.topjohnwu.magisk` | `io.github.huskydg.magisk` |

> 详细的修改文件清单请参考 [docs/MODIFICATIONS.md](docs/MODIFICATIONS.md)

## 项目结构

```
magisk-for-VPhoneOS/
├── app/                          # Android 应用
├── native/src/
│   ├── core/
│   │   ├── deny/                 # MagiskHide（cli、utils、ptrace、revert）
│   │   ├── zygisk/              # Zygisk 模块（hook、module、solist、memory）
│   │   └── kitsune.cpp          # Kitsune Mask 全局变量和函数
│   ├── external/                # 外部依赖（9 个子模块 + xdl + parallel-hashmap）
│   └── include/                 # 头文件
├── scripts/                     # 构建和安装脚本
└── build.py                     # 构建脚本
```

## 技术细节

### 架构说明

本项目是一个**缝合体**：

| 组件 | 来源 | 说明 |
|------|------|------|
| Root 框架 | Magisk 30.7 | Rust 守护进程（magiskd）、模块系统、Zygisk API |
| 隐藏特性 | Kitsune Mask (kokoro-no-kitsune) | MagiskHide、SuList、SoList、内存重映射、ptrace 监控 |
| Zygisk 注入 | Kitsune Mask (kokoro-no-kitsune) | ptrace 方式注入（替代 Magisk 30.7 的 NativeBridge） |
| 守护进程 | Magisk 30.7 | Rust 重写（C++ 版保留为死代码） |

### Zygisk 注入流程

```
POST_FS_DATA 阶段
  → init_monitor 线程启动（magiskd 内）
  → PTRACE_SEIZE init(1) 带 PTRACE_O_TRACEFORK
  → 监听 init fork 的所有子进程
  → 子进程 exec 为 app_process64/32 → inject_zygote():
    1. kill(SIGSTOP) 暂停 zygote
    2. PTRACE_DETACH + 保持 SIGSTOP
    3. xfork() 出子进程
    4. 子进程按架构选择 tracer：
       - app_process64 → execl("/sbin/magisk64", "magisk", "--zygisk", "trace_zygote", pid, libpath)
       - app_process32 → execl("/sbin/magisk32", "magisk", "--zygisk", "trace_zygote", pid, libpath)
    5. 父进程 waitpid(WNOHANG, 5s 超时) 等待退出码
    6. trace_zygote():
       - PTRACE_SEIZE(zygote) 附加
       - EINVAL 降级（kernel < 5.3 无 PTRACE_O_EXITKILL）
       - inject_on_main(): process_vm_readv/writev 远程注入
       - dlopen + dlsym("zygisk_inject_entry")
       - kill(SIGCONT) 恢复 zygote
    7. 退出码 0 = 成功；2+ = 失败步骤
```

### 已知根因与修复（历史）

### 32 位兼容性

本项目采用三二进制规范（参考 Magisk 27.0/kokoro-no-kitsune）：

| 二进制 | 角色 | 架构 |
|--------|------|------|
| `magisk` | 主 CLI（daemon、su、--post-fs-data） | 设备主 ABI |
| `magisk32` | 32 位 Zygisk tracer | 32 位 ABI（arm/x86） |
| `magisk64` | 64 位 Zygisk tracer | 64 位 ABI（arm64/x86_64，与 magisk 同文件，`live_setup.sh` 中创建符号链接） |

`inject_zygote()` 根据 `app_process32`/`app_process64` 自动选择对应 tracer，解决跨架构读栈问题。

### 退出码速查表

trace_zygote 失败时通过 `_exit(code)` 通知父进程，父进程记录 `zygisk: trace_zygote PID %d fail step=%d`：

| 退出码 | 含义 |
|--------|------|
| 2 | `PTRACE_SEIZE` 失败 |
| 3 | `wait_for_trace`（等 SIGSTOP）失败 |
| 4 | 初始 SIGSTOP 状态不对 |
| 51-64 | `inject_on_main` 内部步骤失败：51=get_regs 52=read_argc 53=get_entry 54=write_break 55=wait_breakpoint ... 64=stopped_other |
| 6 | `kill(SIGCONT)` 失败 |
| 7-8 | 后续 ptrace 操作失败 |
| 9 | SIGTRAP 后状态不对 |

### 反检测流程

```
run_modules_pre() — 内存重映射
  └── 对 UID >= 10000 的应用：
      └── mremap 重映射为匿名内存

run_modules_post() — SoList 隐藏
  └── 对 UID >= 10000 的应用：
      └── NullifySoName 清除 Zygisk 模块路径

~ZygiskContext() — 内存释放
  └── munmap 整块内存
```

### VPhoneOS 兼容性

本项目针对 VPhoneOS 虚拟化环境做了特殊适配：

| 场景 | 真机行为 | VPhoneOS 行为 |
|------|---------|--------------|
| **Zygisk 注入** | `PTRACE_SEIZE init(1)` 继承 TRACEFORK 监听 | 回退到 `find_zygote_by_polling()` 扫描 `/proc` |
| **magiskd peer_cred** | 校验 `cred.uid == 0` | 跳过 uid 检查（VPhoneOS SO_PEERCRED 返回 real uid） |
| **su 鉴权** | 通过数据库 + Manager 鉴权 | uid 强制为 0，直接允许 |
| **Zygisk companion** | Rust `zygisk_companion_entry()` 处理 | no-op，由 C++ `zygiskd()` 处理 |
| **Kitsune su_mount** | 执行 `kitsune_load_modules_for_su()` | 跳过模块重载 |

检测方式：检查 `/share` 目录存在性（VPhoneOS sharefs 挂载点）。统一函数 `base::is_vphoneos()` 定义在 `base/files.rs`。

### 进程监控对比

| 方案 | 原理 | 可靠性 | CPU 占用 |
|------|------|--------|----------|
| Logcat（原版 Magisk） | 解析 Android 日志系统 | 可能遗漏 | 较高 |
| ptrace（本项目 & Kitsune） | 附加到 init 监控 fork | 不会遗漏 | 低 |

### 调试构建

默认 `./build.py all` 为 debug 构建（`MAGISK_DEBUG=1`）：
- `ptrace.cpp` 中的 `TRACELOGE`/`TRACELOGW` 宏通过 `trace_log()` 追加到 `/cache/magisk.log`
- init_monitor 的 `waitpid` 轮询捕获 trace_zygote 退出码
- 注入成功显示 `trace_zygote done for PID`；失败显示 `fail step=<code>`

Release 构建（`./build.py -r all`）：
- `TRACELOGE`/`TRACELOGW` 编译为空操作
- `ZLOGD` 编译为空函数（LTO 优化掉）
- `LOGE`/`LOGW`/`LOGI` 保留

## 许可证

GPLv3 - 详见 [LICENSE](LICENSE)
