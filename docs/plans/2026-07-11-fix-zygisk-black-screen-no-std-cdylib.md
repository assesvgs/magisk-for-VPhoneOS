# Zygisk 黑屏修复 —— no_std Rust cdylib 注入方案

> **约束：** 无法本地编译，所有构建走 GitHub CI。每次 push 触发一次 CI 构建（约 10 分钟）。需批量编写代码后统一推送验证。
>
> **更新记录：** 2026-07-12 — 根据实施过程修正计划与实际偏差。核心修复方向不变。

**目标：** 修复 32 位 + 64 位 Zygisk 同时注入后 system_server 崩溃导致的 VPhoneOS 黑屏问题。根因为 dlopen 加载 magisk 二进制时 Rust `std` 的全局构造器（`.init_array`）干扰 ART 进程状态，导致 SIGSEGV 信号处理程序失败。

**架构：** 将注入 zygote 的代码从 magisk 二进制中抽取为一个独立的 Rust `#![no_std]` 库 `libzygisk_inject.so`。ptrace 注入时 dlopen 此 .so 而非整个 magisk 二进制。`no_std` 保证没有 `std` 级别的全局构造器运行。

**技术栈：** Rust（`#![no_std]` + `core` + `alloc`）+ C FFI 调用 lsplt + C++ JNI 操作。

---

## 执行策略：四阶段推进

```
Phase 1  ─── no_std 骨架 + 构建集成
                → 7 轮 CI 迭代定案：staticlib + ndk-build

Phase 2a ─── PLT hook 框架
Phase 2b ─── JNI env->functions 表替换
Phase 2c ─── Hook 回调 + 模块加载
                → 各子阶段独立 CI 验证

Phase 3  ─── 注入路径切换 + 部署 + 清理旧文件
```

## 与实际代码的偏差及根因

| 计划项 | 计划指定 | 实际实现 | 偏差根因 |
|--------|---------|---------|---------|
| `crate-type` | `["cdylib"]` | `["staticlib"]` | cargo 产 `.so` 需要交叉链接器，但本项目 Rust 工具链从
没配过链接器（所有 crate 产 `.a`，ndk-build 做最终链接）。6 轮 CI 失败后改为 staticlib + ndk-build |
| C++ 桥接 | `cxx` crate + `build.rs` 代码生成 | 手动 C FFI（`.h` + `.cpp` + `extern "C"`） | `cxx` 的 `build.rs` 与 staticlib + ndk-build 流程不兼容。手写 FFI 更直接 |
| JNI 类型 | `jni` crate `sys` 模块 | C++ `jni.h`（NDK 原生） | NDK 原生 `jni.h` 保证与目标 Android 版本一致，零依赖风险 |
| `Android-rs.mk` | 需修改 | 未修改 | `.a` 由 `build.py` 直接复制到 `native/out/<arch>/`，`Android.mk` 的 `B_ZYGISK_INJECT` 段直接消费，不需要 Android-rs.mk |
| `ptrace.cpp` | 需修改 dlopen 路径 | 未修改 | `ptrace.cpp::trace_zygote()` 接收 `libpath` 参数，不关心路径来源。路径由 `init_monitor.cpp` 的 `execl()` 最后一个参数传入 |
| `cxx/stubs.cpp`（最小 C++ runtime）| 未在计划中 | 新增 | 替代完整 `libcxx` 以避免 C++ 标准库的 `.init_array` 条目 |
| `context.rs`（HookContext 结构体）| 需创建 | 未创建 | 全局状态用 `static` 变量 + HookSlot 枚举管理，比 C++ 全局对象更安全 |
| 模块加载 | Phase 2c 实现 | 已实现基础框架 | `module.rs` 扫描 `/data/adb/modules/*/zygisk/`，dlopen + 调用 entry |

---

## 文件清单（实际）

### 新增文件

| 文件 | 职责 |
|------|------|
| `native/src/zygisk_inject/Cargo.toml` | Rust 项目配置，staticlib，仅 libc 依赖 |
| `native/src/zygisk_inject/.cargo/config.toml` | `build-std = ["core", "alloc"]` |
| `native/src/zygisk_inject/src/lib.rs` | `#![no_std]`，`zygisk_inject_entry` → `hook_plt()` |
| `native/src/zygisk_inject/src/memory.rs` | mmap 全局分配器 |
| `native/src/zygisk_inject/src/plt.rs` | `/proc/self/maps` 扫描 + lsplt FFI |
| `native/src/zygisk_inject/src/jni.rs` | JNI hook FFI 声明 |
| `native/src/zygisk_inject/src/hooks.rs` | 6 个 PLT hook 回调 + `hook_plt()` 编排 |
| `native/src/zygisk_inject/src/module.rs` | Zygisk 模块加载 |
| `native/src/zygisk_inject/build.rs` | 空（C++ 编译由 ndk-build 处理） |
| `native/src/zygisk_inject/include/plt_hook.h` | lsplt C FFI 头文件 |
| `native/src/zygisk_inject/include/jni_hook.h` | JNI hook C 头文件 |
| `native/src/zygisk_inject/cxx/plt_hook.cpp` | lsplt RegisterHook/CommitHook 封装 |
| `native/src/zygisk_inject/cxx/jni_hook.cpp` | JNI_GetCreatedJavaVMs → mmap 复制 env–>functions |
| `native/src/zygisk_inject/cxx/stubs.cpp` | 最小 C++ runtime（替代 libcxx）|

### 修改文件

| 文件 | 改动 |
|------|------|
| `build.py` | 新增 `build_cdylib()`，在 `build_native()` 中调用 |
| `native/src/Android.mk` | 新增 `B_ZYGISK_INJECT` 段，移除旧 hook.cpp/memory.cpp |
| `app/buildSrc/.../Setup.kt` | `archBins` 添加 `libzygisk_inject.so`，文件数校验 `*8` |
| `native/src/core/zygisk/init_monitor.cpp` | `inject_zygote()` 和 `find_zygote_by_polling()` 的 execl 最后一个参数改为 inject_lib |
| `scripts/live_setup.sh` | 部署列表添加 `libzygisk_inject.so` |

### 删除文件

| 文件 | 原因 |
|------|------|
| `native/src/core/zygisk/hook.cpp` | 功能迁移到 hooks.rs + jni_hook.cpp |
| `native/src/core/zygisk/memory.cpp` | 功能迁移到 memory.rs |
| `native/src/core/zygisk/memory.hpp` | 不再需要 |
| `native/src/core/zygisk/jni_hooks.hpp` | 功能迁移到 jni.rs + jni_hook.cpp |
| `native/src/core/zygisk/gen_jni_hooks.py` | 不再需要 |

---

## 构建与验证

### CI 流程

```
push → GitHub Actions (macOS-15)
  → build.py -vr all（release）
    → build_rust_src(targets) — 父 workspace Rust crate
    → build_cdylib() — 构建 libzygisk_inject.a
    → build_cpp_src(targets) — ndk-build 链接 .so
    → Gradle 打包 APK
  → 产物上传为 artifact
```

### 关键验证步骤

1. `readelf -d libzygisk_inject.so` → 确认 `.init_array` 条目数为 0
2. `readelf --dyn-syms` → 确认 `zygisk_inject_entry` 等符号导出
3. 部署到 VPhoneOS，开启 Zygisk，重启
4. 检查 magisk.log（trace_zygote done + BOOT_COMPLETE）
5. 检查 UserKernel.log（SIGSEGV 被 ART 正常恢复）

### 已知遗留项

| 项 | 说明 | 优先级 |
|----|------|--------|
| specialization 方法拦截 | `hook_RegisterNatives` 已就位但未替换 `nativeForkAndSpecialize` 等函数指针 | 低（需 Android 版本特定签名） |
| 模块 dispatch | 已加载模块但未在 specialize 时调用 | 低（需 specialization 拦截就绪后） |
| `app_specialize_pre/post` | 未实现 | 低（依赖上述两项） |
