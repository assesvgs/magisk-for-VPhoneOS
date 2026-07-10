# Zygisk 黑屏修复 —— no_std Rust cdylib 注入方案

> **约束：** 无法本地编译，所有构建走 GitHub CI。每次 push 触发一次 CI 构建（约 10 分钟）。需批量编写代码后统一推送验证。

**目标：** 修复 32 位 + 64 位 Zygisk 同时注入后 system_server 崩溃导致的 VPhoneOS 黑屏问题。根因为 dlopen 加载 magisk 二进制时 Rust `std` 的全局构造器（`.init_array`）干扰 ART 进程状态，导致 SIGSEGV 信号处理程序失败。

**架构：** 将注入 zygote 的代码从 magisk 二进制中抽取为一个独立的 Rust `#![no_std]` `cdylib` 库 `libzygisk_inject.so`。ptrace 注入时 dlopen 此 .so 而非整个 magisk 二进制。`no_std` 保证没有 `std` 级别的全局构造器运行。

**技术栈：** Rust，`#![no_std]` + `core` + `alloc`，cxx bridge 调用 C++ `lsplt` 库。

---

## 执行策略：三阶段推进

```
Phase 1 ─── 本地写代码：cdylib 骨架 + 分配器 + cxx 桥 + 构建集成
                → push → CI 编译 → 下载产物 → readelf 验证 .init_array
                → 通过则 Phase 2

Phase 2 ─── 本地写代码：PLT hook + JNI hook + HookContext + 模块管理
                → push → CI 编译 → 通过则 Phase 3

Phase 3 ─── 本地写代码：注入路径修改 + 部署脚本 + 清理旧文件
                → push → CI 编译 → 部署到 VPhoneOS 验证
```

## 文件清单

| 文件 | 操作 | 职责 |
|------|------|------|
| `native/src/zygisk_inject/` | 创建 | 新 Rust cdylib 项目（~8 个文件） |
| `native/src/Android.mk` | 修改 | 新增 libzygisk_inject PREBUILT_SHARED_LIBRARY |
| `native/src/Android-rs.mk` | 修改 | 新增 cdylib 条目 |
| `build.py` | 修改 | cdylib 构建 + 产物拷贝 |
| `native/src/core/zygisk/ptrace.cpp` | 修改 | dlopen 路径 |
| `native/src/core/zygisk/init_monitor.cpp` | 修改 | tracer 路径 |
| `scripts/live_setup.sh` | 修改 | 部署 .so |
| `native/src/core/zygisk/hook.cpp` | 删除 | 不再需要 |
| `native/src/core/zygisk/memory.cpp` | 删除 | 不再需要 |
| `native/src/core/zygisk/memory.hpp` | 删除 | 不再需要 |
| `native/src/core/zygisk/jni_hooks.hpp` | 删除 | 不再需要 |
| `native/src/core/zygisk/gen_jni_hooks.py` | 删除 | 不再需要 |

---

### Phase 1：cdylib 骨架（任务 1-3）

**目标：** 创建可编译的 `libzygisk_inject.so`，确认 `.init_array` 为空。

**验证：** CI 构建后下载产物，`readelf -d` 确认 `.init_array` 条目数为 0。

#### 任务 1：创建 cdylib 基础结构

**文件：**
- 创建：`native/src/zygisk_inject/Cargo.toml`
- 创建：`native/src/zygisk_inject/.cargo/config.toml`
- 创建：`native/src/zygisk_inject/src/lib.rs`
- 创建：`native/src/zygisk_inject/build.rs`

`Cargo.toml`：
```toml
[package]
name = "zygisk_inject"
version = "0.1.0"
edition = "2021"
[lib]
crate-type = ["cdylib"]
[dependencies]
cxx = { path = "../external/cxx-rs" }
libc = "0.2"
[build-dependencies]
cxx-build = { path = "../external/cxx-rs/gen/cmd" }
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

`.cargo/config.toml`（空文件，覆盖根目录的 build-std）：
```toml
```

`src/lib.rs`：
```rust
#![no_std]
extern crate alloc;
use core::panic::PanicInfo;
use core::ffi::c_void;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! { loop {} }

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(handle: *mut c_void) {}
#[no_mangle]
pub extern "C" fn zygisk_companion_entry(socket: i32) {}
```

`build.rs`：
```rust
fn main() {
    cxx_build::bridge("src/lib.rs")
        .flag("-std=c++23")
        .flag("-Oz")
        .compile("zygisk_inject_cxx");
    println!("cargo:rerun-if-changed=src/lib.rs");
}
```

#### 任务 2：mmap 分配器

**文件：**
- 创建：`native/src/zygisk_inject/src/memory.rs`

```rust
use core::alloc::{GlobalAlloc, Layout};

struct MmapAllocator;

unsafe impl GlobalAlloc for MmapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(16).next_power_of_two();
        let ptr = libc::mmap(core::ptr::null_mut(), size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
        if ptr == libc::MAP_FAILED { core::ptr::null_mut() }
        else { ptr as *mut u8 }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        libc::munmap(ptr as *mut libc::c_void, layout.size());
    }
}

#[global_allocator]
static ALLOCATOR: MmapAllocator = MmapAllocator;
```

`lib.rs` 追加：`mod memory;`

#### 任务 3：cxx bridge 封装 lsplt

**文件：**
- 创建：`native/src/zygisk_inject/include/zygisk_plt_bridge.hpp`
- 创建：`native/src/zygisk_inject/cxx/zygisk_plt_bridge.cpp`
- 修改：`build.rs`
- 创建：`native/src/zygisk_inject/src/plt.rs`

`include/zygisk_plt_bridge.hpp`：
```cpp
#pragma once
#include <lsplt.hpp>
struct PltHookEntry { const char *symbol; void *hook; void **orig; };
bool register_plt_hook(const PltHookEntry &e);
bool commit_plt_hooks();
```

`build.rs` 补充 lsplt 头文件路径和 C++ 源文件。

`src/plt.rs`：Rust 侧 `PltHook` struct + `register()` / `commit()` 方法。

---

### Phase 2：核心逻辑迁移（任务 4-6）

#### 任务 4：PLT hook 注册

**文件：**
- 修改：`native/src/zygisk_inject/src/plt.rs`
- 修改：`native/src/zygisk_inject/src/lib.rs`

实现所有 PLT hook 注册（fork、unshare、androidSetCreateThreadFunc、selinux_android_setcontext、__android_log_close），对应 hook.cpp 的 PLT_HOOK_REGISTER_SYM 宏。

#### 任务 5：JNI hook

**文件：**
- 创建：`native/src/zygisk_inject/src/jni.rs`
- 修改：`native/src/zygisk_inject/lib.rs`

JNI 类型绑定（JNINativeInterface、JNIEnv、JNINativeMethod 的 repr(C) 结构体），`hook_jni_env()` 复制旧表 → 分配新表 → 替换 RegisterNatives。

#### 任务 6：HookContext 和模块生命周期

**文件：**
- 创建：`native/src/zygisk_inject/src/context.rs`
- 创建：`native/src/zygisk_inject/src/module.rs`

HookContext（env、process、pid、info_flags、modules）、app_specialize_pre/post、server_specialize_pre/post、ZygiskModule 加载。

---

### Phase 3：集成部署与清理（任务 7-9）

#### 任务 7：构建集成

**文件：**
- 修改：`build.py`
- 修改：`native/src/Android-rs.mk`
- 修改：`native/src/Android.mk`

`build.py` 添加 cdylib 编译，`Android-rs.mk` 添加预编译 .so 条目，`Android.mk` 移除旧 C++ 源文件。

#### 任务 8：注入路径 + 部署

**文件：**
- 修改：`native/src/core/zygisk/ptrace.cpp`
- 修改：`native/src/core/zygisk/init_monitor.cpp`
- 修改：`scripts/live_setup.sh`

dlopen 路径改为 `/sbin/libzygisk_inject.so`，部署脚本复制 .so。

#### 任务 9：清理旧文件

**文件：**
- 删除：`native/src/core/zygisk/hook.cpp`
- 删除：`native/src/core/zygisk/memory.cpp`
- 删除：`native/src/core/zygisk/memory.hpp`
- 删除：`native/src/core/zygisk/jni_hooks.hpp`
- 删除：`native/src/core/zygisk/gen_jni_hooks.py`

---

### 任务 10：VPhoneOS 部署验证

- [ ] 从 CI 下载 app-release.apk
- [ ] 部署到 VPhoneOS，开启 Zygisk，重启
- [ ] 检查 magisk.log（trace_zygote done + BOOT_COMPLETE）
- [ ] 检查 UserKernel.log（SIGSEGV 被 ART 正常恢复）
