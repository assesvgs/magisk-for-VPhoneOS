![](docs/images/kitsune_logo.png)

# Kitsune Mask for VPhoneOS

基于 Magisk 30.7，集成 Kitsune Mask 特性，适用于 VPhoneOS 安卓虚拟机环境。

## 项目说明

本项目以 Magisk 30.7 为基础代码，将 Kitsune Mask（已停更的 Magisk 第三方修改版）的核心功能移植进来。Kitsune Mask 27001 版本存在 MagiskHide 无法正常卸载 /sbin 的问题，本项目通过集成 25210 版本的 revert.cpp 解决了此问题。

基础代码来源：`https://github.com/topjohnwu/Magisk.git` tag v30.7

Kitsune Mask 参考来源：
- `KitsuneMag-kitsune`（kitsune 分支，版本 25210）
- `kokoro-no-kitsune-27001b`（版本 27001b）

## 与原版 Magisk 30.7 的差异

### 1. MagiskHide 功能（原版没有）

原版使用 `--denylist` 命令，本项目改为 `--hide` 命令，完整实现如下：

| 功能 | 命令 | 说明 |
|------|------|------|
| 状态查看 | `magisk --hide status` | 查看 MagiskHide 启用状态 |
| 启用/禁用 | `magisk --hide enable/disable` | 开关 MagiskHide |
| 列表管理 | `magisk --hide add/rm/ls` | 管理隐藏列表 |
| 手动卸载 | `magisk --hide --do-unmount` | 手动卸载 Magisk 修改（原版没有） |
| /sbin 挂载 | `magisk --hide --mount-sbin` | 挂载 /sbin（原版没有） |
| /sbin 设置 | `magisk --hide --setup-sbin` | 设置 /sbin（原版没有） |

### 2. SuList 白名单模式（原版没有）

SuList 是与 DenyList（黑名单）相反的白名单模式。启用后，只有列表中的应用才能获取 root 权限。

| 功能 | 命令 |
|------|------|
| 查看状态 | `magisk --hide sulist` |
| 启用 | `magisk --hide sulist enable` |
| 禁用 | `magisk --hide sulist disable` |

### 3. 进程监控方式变更

| 方面 | 原版 Magisk 30.7 | 本项目 |
|------|------------------|--------|
| 方式 | logcat 日志解析 | ptrace 附加 zygote |
| 可靠性 | 可能遗漏进程 | 不会遗漏 |
| CPU 占用 | 较高 | 低（事件驱动） |
| 实现文件 | `deny/logcat.cpp` | `deny/ptrace.cpp` |

### 4. revert_unmount 实现变更

| 方面 | 原版 Magisk 30.7 | 本项目 |
|------|------------------|--------|
| 实现语言 | Rust (`mount.rs`) | C++ (`deny/revert.cpp`) |
| 功能范围 | 基础卸载 | 完整卸载（含 early-mount.d、sbin） |
| 调用入口 | `mount::revert_unmount` | `deny/revert.cpp::revert_unmount` |

### 5. Zygisk 反检测增强（原版没有）

| 功能 | 文件 | 说明 |
|------|------|------|
| SoList 隐藏 | `zygisk/solist.hpp` | 从 linker 内部链表中抹掉 Zygisk 模块路径 |
| 内存重映射 | `zygisk/module.cpp` | 用 mremap 隐藏内存映射，从 /proc/self/maps 中抹掉痕迹 |
| 内存分配器 | `zygisk/memory.cpp/hpp` | mmap 单调分配器，munmap 整块释放，避免内存碎片痕迹 |
| 依赖库 | `external/_xDL` | xdl 库，用于访问 linker 内部符号 |
| 依赖库 | `external/parallel-hashmap` | 高性能哈希表，用于 JNI hook 内存管理 |

### 6. 启动时挂载 MagiskSU（原版没有）

`core/kitsune.cpp` 中实现了 `enable_mount_su()`，在 `boot_complete()` 阶段调用，将 MagiskSU 二进制挂载到系统中。

### 7. 应用信息变更

| 项目 | 原版 | 本项目 |
|------|------|--------|
| 包名 | `com.topjohnwu.magisk` | `io.github.huskydg.magisk` |
| 应用名称 | Magisk | Kitsune Mask |
| 图标 | Magisk 狗头 | Kitsune Mask 狐狸 |
| 设置项 | DenyList | MagiskHide + SuList |

### 8. UI 设置项

| 设置项 | 说明 | 原版 |
|--------|------|------|
| MagiskHide 开关 | 启用/禁用 MagiskHide | 原版为 DenyList |
| SuList 开关 | 启用/禁用白名单模式 | 原版没有 |
| Configure MagiskHide | 配置隐藏列表 | 原版为 Configure DenyList |

## 修改文件清单

### 新增文件（原版没有）

| 文件 | 说明 |
|------|------|
| `core/kitsune.cpp` | Kitsune Mask 全局变量（magisktmpfs_fd、su_bin_fd、HAVE_32、logging_muted）和函数（enable_mount_su、su_mount、mount_mirrors） |
| `core/deny/ptrace.cpp` | ptrace 进程监控实现，替代原版的 logcat.cpp |
| `core/deny/revert.cpp` | 完整的 revert_unmount 实现，包含 /sbin 挂载/卸载 |
| `core/include/selinux.hpp` | SELinux 函数声明（revert.cpp 依赖） |
| `zygisk/solist.hpp` | SoList 隐藏功能 |
| `zygisk/memory.cpp` | JNI hook 内存分配器实现 |
| `zygisk/memory.hpp` | JNI hook 内存分配器头文件 |

### 删除文件（原版有）

| 文件 | 原因 |
|------|------|
| `core/deny/logcat.cpp` | 被 ptrace.cpp 替代 |

### 修改文件

| 文件 | 修改内容 |
|------|---------|
| `native/src/Android.mk` | 构建列表：logcat.cpp → ptrace.cpp + revert.cpp + kitsune.cpp；添加 libphmap、libxdl 依赖 |
| `native/src/external/Android.mk` | 添加 libphmap 和 libxdl 构建规则 |
| `native/src/Application.mk` | 添加 APP_ABI（四架构）、APP_SUPPORT_FLEXIBLE_PAGE_SIZES（16KB 页面）、B_BB（Busybox） |
| `core/include/core.hpp` | 添加 Kitsune Mask 声明（revert_unmount、magisktmpfs_fd、su_bin_fd、logging_muted、su_mount、mount_mirrors） |
| `core/deny/deny.hpp` | 添加 sulist 枚举（SULIST_STATUS、ENFORCE_SULIST、DISABLE_SULIST）和函数声明 |
| `core/deny/cli.cpp` | 命令从 --denylist 改为 --hide，添加 sulist、--do-unmount、--mount-sbin、--setup-sbin |
| `core/deny/utils.cpp` | 添加 sulist 功能、rescan_apps、is_deny_target 三参数版本、proc_monitor 线程启动 |
| `core/lib.rs` | 添加 ZygiskRequest（SulistRootNs、RevertUnmount）、ZygiskStateFlags（ProcessOnAllowList、AllowlistEnforcing、DoRevertUnmount、DoAllow）、enable_mount_su/revert_unmount FFI 声明 |
| `core/magisk.rs` | --denylist 改为 --hide |
| `core/mount.rs` | 删除 Rust 版本 revert_unmount（移至 C++） |
| `core/bootstages.rs` | 添加 enable_mount_su() 调用 |
| `zygisk/entry.cpp` | 添加 remote_request_sulist() 和 remote_request_umount() |
| `zygisk/hook.cpp` | 添加 sulist 逻辑、SoList 隐藏调用、内存释放调用 |
| `zygisk/module.cpp` | 添加 sulist 模式判断、SoList 隐藏、内存重映射隐藏 |
| `zygisk/module.hpp` | 添加 DO_ALLOW 标志 |
| `zygisk/zygisk.hpp` | 添加 sulist 函数声明 |
| `include/consts.hpp` | 包名改为 io.github.huskydg.magisk，添加 Kitsune Mask 常量 |
| `init/bootstages.rs` | 添加 enable_mount_su 导入和调用 |
| `init/rootdir.rs` | 添加 start logd 和 zygote-restart 特性 |
| `app/.../SettingsItems.kt` | 添加 MagiskHide 和 SuList 设置项 |
| `app/.../SettingsViewModel.kt` | 注册 MagiskHide 和 SuList |
| `app/.../Config.kt` | 添加 sulist 配置属性 |
| `app/.../Info.kt` | 添加 sulist 状态检查 |
| `app/.../DenyListFragment.kt` | 根据 sulist 状态切换标题 |
| `app/.../strings.xml` | 添加 MagiskHide、SuList 相关字符串 |
| `app/shared/AndroidManifest.xml` | 应用名称改为 Kitsune Mask |
| `app/core/res/drawable/ic_magisk.xml` | 图标改为 Kitsune Mask 狐狸 |

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

额外依赖（非子模块，需手动下载）：
- `native/src/external/_xDL` — xdl 库（SoList 隐藏依赖）
- `native/src/external/parallel-hashmap` — 高性能哈希表（内存分配器依赖）

### 编译

```bash
./build.py ndk          # 安装 NDK（首次）
./build.py all          # 完整构建
./build.py binary       # 仅构建原生代码
./build.py app          # 仅构建应用
```

CI 构建仅编译 arm64-v8a 架构（`.github/ci.prop`），完整构建支持 armeabi-v7a、arm64-v8a、x86、x86_64。

## 项目结构

```
magisk-for-VPhoneOS/
├── app/                          # Android 应用代码
│   ├── apk/                      # 主应用
│   ├── core/                     # 核心库
│   └── shared/                   # 共享代码
├── native/src/
│   ├── core/
│   │   ├── deny/                 # MagiskHide 实现
│   │   │   ├── cli.cpp           # 命令行接口（--hide）
│   │   │   ├── utils.cpp         # sulist、rescan_apps、is_deny_target
│   │   │   ├── ptrace.cpp        # 进程监控（替代 logcat）
│   │   │   └── revert.cpp        # revert_unmount 完整实现
│   │   ├── zygisk/              # Zygisk 模块
│   │   │   ├── entry.cpp         # sulist/umount 请求入口
│   │   │   ├── hook.cpp          # JNI hook、SoList 隐藏、内存释放
│   │   │   ├── module.cpp        # sulist 模式判断、内存重映射
│   │   │   ├── solist.hpp        # SoList 隐藏实现
│   │   │   ├── memory.cpp/hpp    # JNI hook 内存分配器
│   │   │   └── module.hpp        # DO_ALLOW 标志
│   │   ├── kitsune.cpp           # Kitsune Mask 全局变量和函数
│   │   └── include/
│   │       ├── core.hpp          # Kitsune Mask 声明
│   │       └── selinux.hpp       # SELinux 函数声明
│   ├── init/
│   │   ├── bootstages.rs         # 启动阶段（含 enable_mount_su 调用）
│   │   └── rootdir.rs            # root 目录（含 start logd、zygote-restart）
│   ├── external/                 # 外部依赖
│   │   ├── crt0/                 # C 运行时启动代码
│   │   ├── cxx-rs/               # Rust/C++ FFI 桥接
│   │   ├── libcxx/               # C++ 标准库
│   │   ├── lsplt/                # PLT hook 框架
│   │   ├── lz4/                  # LZ4 压缩
│   │   ├── selinux/              # SELinux 策略
│   │   ├── system_properties/    # Android 系统属性
│   │   ├── _xDL/                 # xdl 库（SoList 依赖）
│   │   └── parallel-hashmap/     # 高性能哈希表
│   └── include/
│       └── consts.hpp            # 包名和常量定义
├── scripts/                      # 构建和安装脚本
├── tools/                        # 工具
├── .github/workflows/build.yml   # CI 构建工作流
└── build.py                      # 构建脚本
```

## 技术细节

### FFI 声明（Rust ↔ C++）

`core/lib.rs` 中的 CXX bridge 声明了以下 Kitsune Mask 函数：

```rust
// unsafe extern "C++" 块中
fn enable_mount_su();                    // kitsune.cpp
fn revert_unmount(pid: i32);             // deny/revert.cpp
fn initialize_denylist();                // deny/utils.cpp
fn update_deny_flags(uid: i32, process: &str, flags: &mut u32);  // deny/utils.cpp
```

```rust
// extern "Rust" 块中（Rust 实现，C++ 调用）
fn revert_unmount(pid: i32);  // 已移至 C++，此声明保留用于 CXX 桥接
```

### Zygisk sulist 流程

```
app_specialize_pre()
  ├── 检查 info_flags 中的 AllowlistEnforcing
  ├── 如果 sulist 模式：
  │   ├── ProcessOnAllowList → 设置 DO_ALLOW 标志
  │   └── 否则 → 设置 DO_REVERT_UNMOUNT 标志
  └── 如果普通 denylist 模式：
      └── 设置 DO_REVERT_UNMOUNT 标志

unshare hook (hook.cpp)
  ├── DO_ALLOW → remote_request_sulist()
  └── DO_REVERT_UNMOUNT → remote_request_umount()
```

### 反检测流程

```
run_modules_pre() — 内存重映射
  └── 对 UID >= 10000 的应用：
      └── 扫描 /proc/self/maps，找到 /memfd:jit-zygisk-cache 和 /modules/
          └── mremap 重映射为匿名内存

run_modules_post() — SoList 隐藏
  └── 对 UID >= 10000 的应用：
      └── SoList::Initialize() 获取 linker 内部符号
          └── NullifySoName 清除 Zygisk 模块路径

~ZygiskContext() — 内存释放
  └── jni_hook::memory_block::release() — munmap 整块内存
```

## 许可证

GPLv3 - 详见 [LICENSE](LICENSE)
