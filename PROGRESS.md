# Zygisk 黑屏修复 — 进度记录

> 基于 `kitsune-mask-integration`，实施于 `ci-clean`

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
| `include/jni_hook.h` + `cxx/jni_hook.cpp` | `dlsym(RTLD_DEFAULT)` → `JNI_GetCreatedJavaVMs` → mmap 复制 env->functions 表 |
| `src/jni.rs` | Rust FFI 声明 |
| `cxx/stubs.cpp` | 最小 C++ runtime 替代 libcxx（消除 .init_array） |

---

## Phase 2c — Hook 回调 + 全局编排 ✅

| 文件 | 说明 |
|------|------|
| `src/hooks.rs` | 6 个 PLT hook 回调（fork/unshare/setcontext/strdup/log_close/dlclose）+ `ORIG_FUNCS` 静态数组 + `hook_plt()` 编排 |
| `src/lib.rs` | `zygisk_inject_entry` 调用 `hook_plt()` + `hook_jni_env()` |

**构建验证：** CI 通过，文件 32KB，`zygisk_inject_entry`/`zygisk_hook_jni_env`/`zygisk_plt_register`/`zygisk_plt_commit` 全部导出。

---

## Phase 3 — 注入路径切换（进行中 ⏳）

| 文件 | 改动 |
|------|------|
| `init_monitor.cpp` | `inject_zygote()` + `find_zygote_by_polling()` 的 execl 最后一个参数改为 inject_lib 路径 |
| `live_setup.sh` | 部署列表添加 `libzygisk_inject.so` |

**推进中：** 旧文件清理（hook.cpp、memory.cpp、jni_hooks.hpp、gen_jni_hooks.py）待验证通过后执行。
