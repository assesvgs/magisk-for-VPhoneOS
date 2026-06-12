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

### 进程监控对比

| 方案 | 原理 | 可靠性 | CPU 占用 |
|------|------|--------|----------|
| Logcat（原版） | 解析 Android 日志系统 | 可能遗漏 | 较高 |
| ptrace（本项目） | 附加到 zygote 监控 fork | 不会遗漏 | 低 |

## 许可证

GPLv3 - 详见 [LICENSE](LICENSE)
