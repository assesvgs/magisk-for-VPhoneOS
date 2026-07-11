# Zygisk 黑屏修复 — 进度记录

## Phase 1 ✅ — no_std cdylib 骨架

**目标：** 创建 `#![no_std]` Rust 注入库，验证无 `.init_array`。

**状态：** ✅ 完成（2026-07-11）

### 改动

| 文件 | 操作 | 说明 |
|------|------|------|
| `native/src/zygisk_inject/` | 新建 | 独立 Rust cdylib 项目 |
| `build.py` | 修改 | 新增 `build_cdylib()`，产 `.a`→ndk-build 链接 |
| `Android.mk` | 修改 | 新增 `B_ZYGISK_INJECT` 段，PREBUILT_STATIC → BUILD_SHARED |
| `Setup.kt` | 修改 | `archBins` 添加 `libzygisk_inject.so`，文件数校验 `*8` |

### CI 失败记录（7 轮）

| 轮 | 根因 | 修复 |
|----|------|------|
| 1 | workspace member 未声明 | `workspace.exclude` |
| 2 | `build-std` 未激活 | `-Z build-std=core` |
| 3~6 | 交叉链接器不可用 | 放弃 cargo 产 `.so`，改 staticlib + ndk-build |
| 7 | `.a` 路径不可达 | 显式 `--target-dir` 传递给 cargo |

### 验证结果

| 架构 | `.init_array` | 导出符号 |
|------|--------------|---------|
| arm64-v8a | **0 条目** ✅ | `zygisk_inject_entry` ✅ |
| armeabi-v7a | **0 条目** ✅ | `zygisk_inject_entry` ✅ |
| x86 | **0 条目** ✅ | `zygisk_inject_entry` ✅ |
| x86_64 | **0 条目** ✅ | `zygisk_inject_entry` ✅ |

---

## Phase 2a — PLT Hook 注册 ✅

**状态：** CI 建设中

### 改动

| 文件 | 说明 |
|------|------|
| `src/memory.rs` | mmap 全局分配器 |
| `src/plt.rs` | `/proc/self/maps` 扫描 + PLT hook FFI |
| `include/plt_hook.h` | lsplt C FFI 头文件 |
| `cxx/plt_hook.cpp` | lsplt RegisterHook/CommitHook 封装 |
| `Android.mk` | 添加 liblsplt + libcxx |

---

## Phase 2b — JNI Hook（进行中）

**目标：** 移植 PLT hook（fork、unshare、selinux_android_setcontext、strdup、__android_log_close、dlclose）

**方案：** C FFI wrapper 调用 lsplt，Rust 侧 extern "C" 调用

---

## Phase 2b — JNI Hook

**目标：** env->functions 表替换

---

## Phase 2c — HookContext + 模块生命周期

**目标：** app_specialize_pre/post、server_specialize_pre/post、ZygiskModule 加载
