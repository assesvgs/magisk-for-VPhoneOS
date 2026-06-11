![](docs/images/kitsune_logo.png)

# Kitsune Mask for VPhoneOS

基于 Magisk 30.7，集成 Kitsune Mask 特性，适用于 VPhoneOS 安卓虚拟机环境。

## 功能特性

### MagiskHide

- 进程级 Magisk 修改隐藏
- 白名单模式（SuList）
- 手动卸载 Magisk 修改（`--do-unmount`）
- /sbin 挂载/卸载支持
- ptrace 进程监控（替代 logcat，更可靠）

### Zygisk 反检测

- SoList 隐藏（从 linker 链表中抹掉 Zygisk 模块记录）
- 内存重映射隐藏（从 /proc/self/maps 中抹掉内存映射）
- JNI hook 内存分配器（mmap 单调分配器，避免内存碎片痕迹）

### 应用信息

- 包名：`io.github.huskydg.magisk`
- 应用名称：Kitsune Mask

## 构建

### 环境要求

- Python 3.8+
- JDK 21
- Android NDK（通过 `./build.py ndk` 自动安装）
- macOS / Linux / Windows

### 子模块

```bash
git submodule update --init --recursive
```

### 编译

```bash
# 完整构建
./build.py all

# 仅构建原生代码
./build.py binary

# 仅构建应用
./build.py app
```

## 命令参考

### MagiskHide

```bash
magisk --hide status              # 查看状态
magisk --hide enable              # 启用
magisk --hide disable             # 禁用
magisk --hide add <pkg> [proc]    # 添加到隐藏列表
magisk --hide rm <pkg> [proc]     # 从隐藏列表移除
magisk --hide ls                  # 查看隐藏列表
magisk --hide --do-unmount        # 手动卸载 Magisk 修改
magisk --hide --mount-sbin        # 挂载 /sbin
magisk --hide --setup-sbin        # 设置 /sbin
```

### SuList

```bash
magisk --hide sulist              # 查看状态
magisk --hide sulist enable       # 启用白名单模式
magisk --hide sulist disable      # 禁用白名单模式
```

## 项目结构

```
magisk-for-VPhoneOS/
├── app/                          # Android 应用
├── native/src/
│   ├── core/
│   │   ├── deny/                 # MagiskHide（cli、utils、ptrace、revert）
│   │   ├── zygisk/              # Zygisk 模块（hook、module、solist、memory）
│   │   └── kitsune.cpp          # Kitsune Mask 全局变量和函数
│   ├── external/                # 外部依赖（7 个子模块 + xdl + parallel-hashmap）
│   └── include/                 # 头文件
├── scripts/                     # 构建和安装脚本
└── tools/                       # 工具
```

## 技术说明

### 进程监控

使用 ptrace 替代原始 Magisk 的 logcat 方案：

| 方案 | 原理 | 可靠性 | CPU 占用 |
|------|------|--------|----------|
| Logcat | 解析 Android 日志系统 | 可能遗漏 | 较高 |
| ptrace | 附加到 zygote 监控 fork | 不会遗漏 | 低 |

### revert_unmount

使用 C++ 实现替代原始 Rust 版本，功能更完整：

- 卸载 magisk tmpfs
- 卸载 worker mounts
- 卸载 module bind mounts
- 卸载 early-mount.d files

## 许可证

GPLv3 - 详见 [LICENSE](LICENSE)
