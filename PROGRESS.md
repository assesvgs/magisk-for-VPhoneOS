# Zygisk 黑屏修复 — 进度记录

> 基于 `kitsune-mask-integration`，实施于 `ci-clean`

## 交接摘要

阅读 `kitsune-mask-integration` 分支上的 `Kitsune_Mask_项目全量分析报告.md` 和 `docs/plans/2026-07-11-fix-zygisk-black-screen-no-std-cdylib.md`（`ci-clean` 分支不包含它们）。当前分支有本 `PROGRESS.md` 记录完整进展。

**黑屏根因：** 注入时 dlopen 的 magisk 二进制含 Rust `std` 的 `.init_array`，在 `zygisk_inject_entry` 运行前就干扰了 ART。纯 C++ 版无此问题；`hook_functions` 设空函数后仍然崩溃。修复方向：`#![no_std]` 独立注入库。

**构建方式：** 本项目所有 Rust crate 只产 `.a`（staticlib），最终链接由 ndk-build 完成。cargo 未配置交叉链接器。最终方案：cargo 产 `.a` → ndk-build 链接成 `.so`。

**部署文件命名：** 部署脚本会把 `libxxx.so` 重命名为 `xxx`，所以文件在 `/sbin/` 中叫 `zygisk_inject` 而非 `libzygisk_inject.so`。前两次部署失败均因路径不一致 + `bootstages.rs` 漏部署。

**进展：** Phase 1~3 代码已完成并 CI 通过。最新提交 `fd27feb`。部署全链路已修复。剩余 specialization 拦截、模块 dispatch、`app_specialize_pre/post`。日志在 `logs/` 下，用 `decrypt.py` 解密。

---

## Phase 1 — no_std 骨架 ✅

**目标：** 创建 `#![no_std]` Rust 注入库，验证 `.init_array` 为空。

**7 轮 CI 迭代后架构定型：** Rust 产 `.a` (staticlib) → ndk-build 产 `.so` (BUILD_SHARED_LIBRARY)。

**结果：** 4 架构均 0 `.init_array`，`zygisk_inject_entry` 导出。根因（Rust std 构造器）被实验证实。

---

## Phase 2a — PLT Hook 框架 ✅

| 文件 | 说明 |
|------|------|
| `src/memory.rs` | mmap 全局分配器 |
| `src/plt.rs` | `/proc/self/maps` 扫描 + lsplt RegisterHook/CommitHook FFI |
| `include/plt_hook.h` + `cxx/plt_hook.cpp` | C FFI 封装 |

---

## Phase 2b — JNI Hook 框架 ✅

| 文件 | 说明 |
|------|------|
| `src/jni_env.rs` | 纯 Rust JNI env 表替换（运行时搜索 RegisterNatives） |
| `cxx/stubs.cpp` | 最小 C++ runtime 替代 libcxx |

---

## Phase 2c — Hook 回调 + 模块加载 ✅

| 文件 | 说明 |
|------|------|
| `src/hooks.rs` | 6 个 PLT hook 回调 + `AtomicBool` ZygoteInit 检测 + `hook_plt()` 编排 |
| `src/module.rs` | Zygisk 模块扫描（`/data/adb/modules/*/zygisk/`） |
| `src/lib.rs` | `zygisk_inject_entry` 调用 `hook_plt()` |

**构建验证：** 32KB，所有符号导出。代码审查修复 8 项（transmute、AtomicBool、null 检查、mprotect、ABI guard 等）。

---

## Phase 3 — 注入路径 + 部署 ✅

| 文件 | 改动 |
|------|------|
| `init_monitor.cpp` | execl 最后一个参数改为 `zygisk_inject` 路径 |
| `live_setup.sh` | 部署列表添加 `zygisk_inject` |
| `bootstages.rs` | 开机时复制 `zygisk_inject` 到 `/sbin/` |
| `Android.mk` | 移除旧 hook.cpp/memory.cpp 引用 |
| `entry.cpp` | 添加 `hook_functions()` 空桩 |
| `jni_hooks.hpp`（根目录） | 删除（孤立文件） |
| `hook.cpp`、`memory.cpp/hpp` | 删除（功能迁移到 cdylib） |
| `gen_jni_hooks.py` | 删除（不再需要） |

**部署全链路：**
```
APK(lib/arm64-v8a/libzygisk_inject.so)
  → MagiskInstaller.kt ($installDir/zygisk_inject)
  → 安装脚本 (/data/adb/magisk/zygisk_inject)
  → bootstages.rs (/sbin/zygisk_inject)
  → init_monitor.cpp dlopen(/sbin/zygisk_inject)
```

---

## VPhoneOS 部署验证

| 尝试 | 提交 | 结果 | 根因 |
|------|------|------|------|
| 第 1 次 | `eb1aec4f` | step=60 注入失败 | `/sbin/libzygisk_inject.so` 不存在 |
| 第 2 次 | `dd80603c` | step=60 注入失败 | `bootstages.rs` 未部署到 `/sbin/` |
| 第 3 次 | `5759d68` | ⏳ 待验证 | 全链路已修复 |

---

## 遗留项

| 项 | 说明 | 优先级 |
|----|------|--------|
| specialization 方法拦截 | `hook_RegisterNatives` 已就位但未替换方法指针 | 低 |
| 模块 dispatch | 已加载模块但未在 specialize 时调用 | 低 |
| `app_specialize_pre/post` | 未实现 | 低 |
