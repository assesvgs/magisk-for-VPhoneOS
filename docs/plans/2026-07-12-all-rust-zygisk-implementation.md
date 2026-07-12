# 全 Rust Zygisk 注入库实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 `subagent-driven-development`（推荐）或 `executing-plans` 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 将 `zygisk_inject` 从当前骨架（PLT hook + JNI env 表替换骨架 ~15% 功能）补全为功能完整的 Zygisk 注入库。全部用 Rust `#![no_std]` 实现，零 C++ 运行时。

**架构：** 保留现有的 PLT hook 框架 + mmap 分配器 + maps 扫描。所有业务逻辑在 Rust 中实现。`gen_jni_hooks.py` 改生成 Rust `extern "C"` 函数，按 JNI 签名字符串匹配。

**技术栈：** Rust nightly（`#![no_std]` + `#![feature(naked_functions)]`） + `core` + `alloc` + `libc` 0.2。lsplt 预编译静态库通过 ndk-build 链接。保留 `cxx/stubs.cpp`（36 行，供 lsplt 链接）和 `cxx/plt_hook.cpp`（10 行，lsplt FFI 包装）。

---

## 文件清单

### 已有文件（保持或修改）

| 文件 | 职责 | 改动 |
|------|------|------|
| `src/lib.rs` | 入口 + panic handler | 扩展入口；加 `#![feature(naked_functions)]` |
| `src/memory.rs` | mmap GlobalAlloc | **不变** |
| `src/plt.rs` | /proc/self/maps 扫描 + lsplt FFI | **不变** |
| `src/hooks.rs` | PLT hook 回调 + HOOK_LIST | 扩展：新增 androidSetCreateThreadFunc、pthread_attr_destroy hook；每个回调实现完整业务逻辑 |
| `src/module.rs` | 模块加载框架 | 重写：修复入口签名 `fn(ZygiskModuleApi*, JNIEnv*) -> i32`；API 表填充 + 模块生命周期 dispatch |
| `src/jni.rs` | JNI hook FFI 声明 | **删除**，功能移到 `jni_env.rs` |
| `Cargo.toml` | 项目配置 | **不变** |

### 新建文件

| 文件 | 职责 | 行数预估 |
|------|------|---------|
| `src/jni_env.rs` | JNI env 表替换（运行时搜索 RegisterNatives） | ~180 |
| `src/hook_context.rs` | HookContext + 全局指针 AtomicPtr + 14 个生命周期方法 | ~380 |
| `src/fd.rs` | FD 管理：FdSet、record_open_fds、sanitize_fds | ~80 |
| `src/ipc.rs` | Daemon IPC：mountinfo 解析→$MAGISKTMP、socket、send_fd/recv_fd | ~200 |
| `src/module_api.rs` | 模块 API 表（ModuleApiV1/2/4）+ 框架→模块回调表（ZygiskModuleImpl） | ~200 |
| `src/proxy_gen.rs` | gen_jni_hooks.py 生成的 JNI 代理函数（19 个）+ JNI_METHOD_TABLE + 签名匹配 | ~500 |
| `src/unload.rs` | 自卸载：unhook_functions + musttail asm | ~80 |
| `src/solist.rs` | SoList 遍历 + NullifySoName + memfd mremap 覆盖 | ~120 |

### 删除文件

| 文件 | 原因 |
|------|------|
| `cxx/jni_hook.cpp` | 功能移到 `jni_env.rs` |
| `include/jni_hook.h` | 不再需要 |
| `src/jni.rs` | 功能移到 `jni_env.rs` |

---

## 架构设计

### 关键架构决策（第二轮审查后修正）

#### ADR 1: JNI env 表替换 — 运行时搜索而非编译期偏移

`build.rs` 保持 `fn main() {}`。JNI env 表替换改为运行时动态搜索：

```
hook_jni_env():
  1. dlsym(RTLD_DEFAULT, "JNI_GetCreatedJavaVMs")
  2. JavaVM→GetEnv → JNIEnv
  3. 读取 env→functions 指针 → 保存为 orig_functions
  4. mmap 4KB → memcpy 原始表到新表（步长 sizeof(usize)，最多 256 条目）
  5. dlsym(RTLD_DEFAULT, "JNI_RegisterNatives") 获取其地址（一次调用）
  6. 遍历新表条目，逐个比较是否等于该地址
  7. 找到后保存 orig_fn 并替换为 hook register_natives
  8. env→functions = 新表；mprotect 只读
  9. 若 dlsym 失败：回退到预计算偏移表（#[cfg] 分架构）
```

#### ADR 2: 模块有两张表——API 表 + 回调表（修复 C1）

模块交互是双向的：
- **模块→框架**（`ModuleApiV1/2/4`）：模块调框架的函数（`hookJniNativeMethods`、`pltHookRegister`、`connectCompanion` 等）
- **框架→模块**（`ZygiskModuleImpl`）：框架触发模块的回调（`preAppSpecialize`、`postAppSpecialize` 等）

`ZygiskModuleApi.base` 指向 `ZygiskModuleImpl` 实例，框架在生命周期事件中通过 `api→base→preAppSpecialize(api)` 调用模块。

#### ADR 3: RegisterNatives 替换顺序（修复 C2）

**先替换方法指针，再调 `orig()`。** ART 在 `orig RegisterNatives` 中读取 `methods[]` 数组并复制函数指针。如果替换发生在 `orig()` 之后，替换无效。

```
hook_register_natives:
  1. 获取类名
  2. if 类 == Zygote: 遍历 methods[]，匹配 JNI_METHOD_TABLE → 替换 fnPtr
  3. orig(env, clazz, methods, nMethods)  // ART 读走已替换的指针
```

#### ADR 4: 模块入口传有效 JNIEnv（修复 C3）

模块入口 `zygisk_module_entry(api, env)` 需要有效 JNIEnv 做初始化（类查找、方法注册）。通过全局 `JavaVM*`（在 `hook_jni_env` 中保存）调用 `GetEnv` 获取。

```rust
static GLOBAL_JAVA_VM: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
// 在 hook_jni_env 成功时保存
GLOBAL_JAVA_VM.store(vm as *mut c_void, Ordering::Release);

pub fn get_global_jni_env() -> *mut c_void {
    let vm = GLOBAL_JAVA_VM.load(Ordering::Acquire);
    if vm.is_null() { return core::ptr::null_mut(); }
    // 通过 JavaVM GetEnv 获取 JNIEnv
}
```

#### ADR 5: 子进程 post-specialize 归属（修复 C4）

Zygote fork 模型中，`nativeForkAndSpecialize` 的原始函数在**子进程中不返回**。子进程直接进入 app 初始化，从 `nativeSpecializeAppProcess` 开始。

因此：
- `nativeForkAndSpecialize_post`：仅运行在**父进程**，做父进程侧清理（FD 清理等）
- `nativeSpecializeAppProcess_pre/post`：运行在**子进程**，承载模块 post-specialize、SoList 隐藏、ZYGISK_ENABLED 环境变量

HookContext 通过 COW 语义在 fork 后对子进程可见。fork 后的子进程通过 PLT hook 的 `fork_post` 或通过 `nativeSpecializeAppProcess` 的 pre 阶段获取 context。

消除 `pre_forked_pid` 机制——PLT fork hook 始终执行原始 fork。

#### ADR 6: JNINativeMethod 用 `*mut` 而非 `*const`（修复 I2）

`methods` 参数声明为 `*mut JNINativeMethod`，因为需要替换其 `fnPtr` 字段。ART JNI 规范的 `RegisterNatives` 接口不保证 methods 数组可写，但实际 ART 实现传递的是可写指针。这是已知的不安全假设，与 C++ 版一致。

### PLT Hook 回调升级

```
new_fork:
  if let Some(ctx) = hook_context::current_ctx():
    ctx.fork_pre() → orig_fork()
    ctx.pid = 返回的 pid
    ctx.fork_post()  // 仅父进程运行
  else: orig_fork()

new_unshare:
  orig_fn(flags) → if CLONE_NEWNS && res==0 && let Some(ctx) = current_ctx():
    if DO_ALLOW: ipc::request_sulist()
    elif !ALLOWLIST_ENFORCED && DO_REVERT_UNMOUNT: ipc::request_umount()
    orig_fn(CLONE_NEWNS)  // 二次 unshare 修复挂载 ID 空洞
    if RESTORE_MOUNT_EXTERNAL_NONE: args.app.mount_external = 0

new_selinux_android_setcontext:
  zygisk_get_logd() → orig_fn(...)

new_strdup:
  不变（ZygoteInit 检测 → jni_env::hook_jni_env）

new_android_log_close:
  if current_ctx()?.flags[SKIP_CLOSE_LOG_PIPE]: return
  else: orig_fn()

新增 androidSetCreateThreadFunc:
  jni_env::hook_jni_env() 早期触发

新增 pthread_attr_destroy:
  if SHOULD_UNLOAD: unhook_functions() → dlclose_self()
  else: orig_fn()
```

### HookContext 设计

```rust
static CURRENT_CTX: AtomicPtr<HookContext> = AtomicPtr::new(core::ptr::null_mut());
pub fn current_ctx() -> Option<&'static mut HookContext> { ... }

pub struct HookContext {
    pub env: *mut c_void,
    pub args: *mut c_void,       // 指向 AppSpecializeArgs 或 ServerSpecializeArgs
    pub process_name: Option<alloc::ffi::CString>,
    pub pid: i32,
    pub flags: Flags,             // bitset 包装 (u16)
    pub info_flags: u32,
    pub modules: alloc::vec::Vec<ZygiskModule>,
    pub allowed_fds: FdSet,
    pub exempted_fds: alloc::vec::Vec<i32>,
}

impl HookContext {
    pub fn new(env: *mut c_void, args: *mut c_void, ...) -> Self { /* 设 CURRENT_CTX */ }
    pub fn fork_pre(&mut self) { /* SIGCHLD 阻塞 + FD 记录 */ }
    pub fn fork_post(&mut self) { /* SIGCHLD 解除阻塞 */ }
    pub fn app_specialize_pre(&mut self)  { /* IPC + flags + 模块加载 */ }
    pub fn app_specialize_post(&mut self) { /* 模块 post + SoList + FD 清理 */ }
    pub fn server_specialize_pre(&mut self) { ... }
    pub fn server_specialize_post(&mut self) { ... }
    pub fn native_fork_and_specialize_pre(&mut self) { ... }
    // native_fork_and_specialize_post 仅父进程运行
    pub fn native_fork_and_specialize_post(&mut self) { /* FD 清理 */ }
    // native_specialize_app_process 在子进程中运行
    pub fn native_specialize_app_process_pre(&mut self) { ... }
    pub fn native_specialize_app_process_post(&mut self) { /* 模块 post + SoList + ZYGISK_ENABLED */ }
    pub fn native_fork_system_server_pre(&mut self) { ... }
    pub fn native_fork_system_server_post(&mut self) { ... }
    pub fn run_modules_pre(&mut self, fds: &[i32]) { ... }
    pub fn run_modules_post(&mut self) { ... }
    pub fn sanitize_fds(&mut self) { /* 遍历 allowed_fds + exempted_fds 关闭其余 */ }
}

impl Drop for HookContext { /* 清 CURRENT_CTX + 设 SHOULD_UNLOAD */ }
```

---

## 实施阶段

### Phase 4 — JNI env 全 Rust 化 + 运行时搜索

**目标：** 删除 `cxx/jni_hook.cpp`，用纯 Rust `jni_env.rs` 实现 JNI env 表替换。

**文件：**
- 新建：`src/jni_env.rs`
- 新建：`src/unload.rs`（stub）
- 新建：`src/module_api.rs`（stub）
- 新建：`src/hook_context.rs`（仅 current_ctx 空桩）
- 修改：`src/lib.rs`（加 feature gate）
- 修改：`src/hooks.rs`（增加 androidSetCreateThreadFunc）
- 删除：`cxx/jni_hook.cpp`、`include/jni_hook.h`、`src/jni.rs`

- [ ] **步骤 1：创建 `src/jni_env.rs`——运行时搜索 RegisterNatives**

```rust
static GLOBAL_JAVA_VM: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static ORIG_FUNCTIONS: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static ORIG_REGISTER_NATIVES: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

pub fn hook_jni_env() -> bool {
    // 1. dlsym JNI_GetCreatedJavaVMs
    let get_vms: Option<unsafe extern "C" fn(*mut *mut c_void, i32, *mut i32) -> i32> = ...;
    // 2. GetEnv
    // 3. 保存 JavaVM 到 GLOBAL_JAVA_VM
    // 4. 保存 orig_functions = env->functions
    // 5. mmap 4KB → memcpy orig → new
    // 6. dlsym("JNI_RegisterNatives") — 一次调用
    // 7. 遍历 new 表（最多 256 个 usize），比对 dlsym 结果
    // 8. 找到 → 保存 orig → 替换为 hook_register_natives
    // 9. env->functions = new; mprotect(PROT_READ)
    true
}

pub fn get_global_jni_env() -> *mut c_void {
    // 通过 GLOBAL_JAVA_VM→GetEnv 获取
}
```

- [ ] **步骤 2：更新 `lib.rs`——feature gate + 入口**

```rust
#![no_std]
#![feature(naked_functions)]
extern crate alloc;
mod memory; mod plt; mod jni_env; mod hooks; mod module;
mod hook_context; mod fd; mod ipc; mod module_api;
mod proxy_gen; mod unload; mod solist;

#[panic_handler] fn panic(_: &PanicInfo) -> ! { unsafe { libc::abort() } }

#[no_mangle]
pub extern "C" fn zygisk_inject_entry(handle: *mut c_void) {
    hooks::install_hooks(handle);
}
```

- [ ] **步骤 3：扩展 `hooks.rs`——install_hooks 保存 self_handle + 增加 androidSetCreateThreadFunc**

- [ ] **步骤 4：更新 `Android.mk`——移除 jni_hook.cpp**

- [ ] **步骤 5：CI 构建 + readelf 验证**

```bash
readelf -d lib/arm64-v8a/libzygisk_inject.so | grep INIT_ARRAY  # 预期：无输出
```

- [ ] **步骤 6：Commit**

```
git commit -m "Phase 4: Rustify JNI env 表替换——运行时搜索 RegisterNatives"
```

---

### Phase 5 — HookContext + FD 管理

**文件：**
- 新建：`src/hook_context.rs`（完整 struct + 14 个方法空桩）
- 新建：`src/fd.rs`（完整实现）

- [ ] **步骤 1：创建 `src/fd.rs`——FdSet + record + sanitize**

```rust
pub const MAX_FD_SIZE: usize = 1024;
pub struct FdSet { bits: [u64; MAX_FD_SIZE / 64] }
impl FdSet {
    pub fn new() -> Self;
    pub fn add(&mut self, fd: i32);
    pub fn contains(&self, fd: i32) -> bool;
    pub fn remove(&mut self, fd: i32);
    pub fn iter(&self) -> FdIter;
}
pub fn record_open_fds(fds: &mut FdSet) { /* 遍历 /proc/self/fd */ }
pub fn sanitize_fds(allowed: &FdSet, exempted: &[i32], pid_fds: &FdSet);
```

- [ ] **步骤 2：创建 `src/hook_context.rs`——完整 struct + 全局指针**

```rust
pub struct Flags(u16);
impl Flags {
    pub const POST_SPECIALIZE: u16            = 1 << 0;
    pub const APP_FORK_AND_SPECIALIZE: u16    = 1 << 1;
    pub const APP_SPECIALIZE: u16             = 1 << 2;
    pub const SERVER_FORK_AND_SPECIALIZE: u16 = 1 << 3;
    pub const DO_REVERT_UNMOUNT: u16          = 1 << 4;
    pub const SKIP_CLOSE_LOG_PIPE: u16        = 1 << 5;
    pub const DO_ALLOW: u16                   = 1 << 6;
    pub const ALLOWLIST_ENFORCED: u16         = 1 << 7;
    pub const RESTORE_MOUNT_EXTERNAL_NONE: u16 = 1 << 8;
    pub const DO_FUTILE_HIDE: u16             = 1 << 9;
    pub const DO_ALLOW_SU: u16                = 1 << 10;
}

static CURRENT_CTX: AtomicPtr<HookContext> = AtomicPtr::new(core::ptr::null_mut());
pub fn current_ctx() -> Option<&'static mut HookContext>;

pub struct HookContext { /* env, args, process_name, pid, flags, info_flags, modules, allowed_fds, exempted_fds */ }

impl HookContext {
    pub fn new(env: *mut c_void, args: *mut c_void, ...) -> Self;  // 设 CURRENT_CTX
    pub fn fork_pre(&mut self);   // SIGCHLD 阻塞 + FD 记录
    pub fn fork_post(&mut self);  // SIGCHLD 解除
    pub fn app_specialize_pre(&mut self);   // 空桩（Phase 6 填充）
    pub fn app_specialize_post(&mut self);  // 空桩（Phase 9 填充）
    pub fn server_specialize_pre/post(&mut self);       // 空桩
    pub fn native_fork_and_specialize_pre(&mut self);   // 空桩
    pub fn native_fork_and_specialize_post(&mut self) { /* 仅父进程→FD 清理 */ }
    pub fn native_specialize_app_process_pre/post(&mut self); // 空桩（子进程）
    pub fn native_fork_system_server_pre/post(&mut self);  // 空桩
    pub fn run_modules_pre(&mut self, fds: &[i32]);   // 空桩（Phase 7 填充）
    pub fn run_modules_post(&mut self);  // 空桩（Phase 7 填充）
    pub fn sanitize_fds(&mut self);
}
impl Drop for HookContext { /* 清 CURRENT_CTX */ }
```

**符号存根**：所有空桩方法提供一个 `/* TODO Phase N */` 注释体，确保编译通过。

- [ ] **步骤 3：CI 构建 + Commit**

```
git commit -m "Phase 5: HookContext + FD 管理 + 全局指针"
```

---

### Phase 6 — Daemon IPC

**文件：**
- 新建：`src/ipc.rs`

- [ ] **步骤 1：实现底层 IPC 函数**

```rust
// 核心设计：
// 1. get_magisk_tmp() 解析 /proc/self/mountinfo 找 "/sbin/.magisk" 挂载点
//    读取所有行，搜索含 ".magisk" 的行，提取挂载路径前缀
// 2. connect_daemon() → AF_UNIX SOCK_STREAM → connect($MAGISKTMP/magiskd)
// 3. send_fd/recv_fd → libc::sendmsg/recvmsg + libc::CMSG_SPACE + SCM_RIGHTS
// 4. read_int/write_int → 固定 4 字节
// 5. read_string/write_string → 长度前缀 + 内容
// 6. remote_get_info → connect → write(GetInfo + uid + process + bitness)
//    → read(info_flags) → read(fd_count) → recv_fd × count

pub fn get_magisk_tmp() -> Option<alloc::string::String>;
pub fn connect_daemon() -> Option<i32>;
pub fn send_fd(sock: i32, fd: i32) -> bool;
pub fn recv_fd(sock: i32) -> Option<i32>;
pub fn read_int(fd: i32) -> Option<i32>;
pub fn write_int(fd: i32, val: i32) -> bool;
pub fn read_string(fd: i32) -> Option<alloc::string::String>;
pub fn write_string(fd: i32, s: &str) -> bool;
pub fn remote_get_info(uid: i32, process: &str) -> Option<(u32, alloc::vec::Vec<i32>)>;
pub fn request_sulist() -> Option<i32>;
pub fn request_umount() -> Option<i32>;
pub fn connect_companion(client: i32) -> bool;
```

- [ ] **步骤 2：集成到 `hook_context.rs`——填充 `app_specialize_pre`**

```rust
pub fn app_specialize_pre(&mut self) {
    let uid = unsafe { *((*self.args).uid) };  // 从 AppSpecializeArgs 读取
    let name = self.process_name.as_deref().unwrap_or("");
    if let Some((info_flags, fds)) = ipc::remote_get_info(uid, name) {
        self.info_flags = info_flags;
        if info_flags & PROCESS_ON_DENYLIST != 0 {
            self.flags.set(Flags::DO_REVERT_UNMOUNT);
        }
        if info_flags & PROCESS_ON_ALLOWLIST != 0 {
            self.flags.set(Flags::DO_ALLOW);
        }
        if info_flags & ALLOWLIST_ENFORCED != 0 {
            self.flags.set(Flags::ALLOWLIST_ENFORCED);
        }
        self.run_modules_pre(&fds);
    }
}
```

- [ ] **步骤 3：Commit**

```
git commit -m "Phase 6: Daemon IPC——socket + mountinfo 解析 + remote_get_info"
```

---

### Phase 7 — 模块系统

**文件：**
- 修改：`src/module_api.rs`（从 stub 变完整）
- 重写：`src/module.rs`

- [ ] **步骤 1：创建 `src/module_api.rs`——API 表 + 回调表**

```rust
#[repr(C)]
pub struct ZygiskModuleApi {
    pub base: *mut c_void,       // → ZygiskModuleImpl
    pub impl_size: u32,
    pub module: *mut c_void,     // → 模块的 API 结构
}

// 框架→模块 的回调表（模块生命周期）
#[repr(C)]
pub struct ZygiskModuleImpl {
    pub pre_app_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub post_app_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub pre_server_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
    pub post_server_specialize: Option<unsafe extern "C" fn(api: *mut c_void)>,
}

// 模块→框架 的 API 表（模块调框架）
#[repr(C)]
pub struct ModuleApiV1 {
    pub handle: *mut c_void,
    pub hook_jni_native_methods: Option<
        unsafe extern "C" fn(*mut c_void, *const libc::c_char, *mut JNINativeMethod, i32) -> bool
    >,
    pub plt_hook_register: Option<
        unsafe extern "C" fn(u64, u64, *const i8, *mut c_void, *mut *mut c_void) -> bool
    >,
    pub plt_hook_exclude: Option<unsafe extern "C" fn(u64, u64, *const i8) -> bool>,
    pub plt_hook_commit: Option<unsafe extern "C" fn() -> bool>,
    pub connect_companion: Option<unsafe extern "C" fn(i32)>,
    pub set_option: Option<unsafe extern "C" fn(u32)>,
}

#[repr(C)]
pub struct ModuleApiV2 { pub v1: ModuleApiV1, pub get_module_dir: ..., pub get_flags: ... }
#[repr(C)]
pub struct ModuleApiV4 { pub v2: ModuleApiV2, pub plt_hook_register_v4: ..., pub plt_hook_commit_v4: ..., pub exempt_fd: ... }

// 填充函数
impl ModuleApiV1 {
    pub fn populate(table: &mut ModuleApiV1) {
        table.hook_jni_native_methods = Some(hook_jni_native_methods_impl);
        table.plt_hook_register = Some(/* lsplt::RegisterHook wrapper */);
        table.plt_hook_exclude = Some(/* lsplt::ExcludeHook wrapper */);
        table.plt_hook_commit = Some(/* lsplt::CommitHook wrapper */);
        table.connect_companion = Some(connect_companion_impl);
        table.set_option = Some(set_option_impl);
    }
}

// 模块生命周期 dispatch 辅助
pub fn call_pre_app_specialize(api_handle: *mut c_void) {
    let api = api_handle as *mut ZygiskModuleApi;
    let impl_ptr = unsafe { (*api).base as *mut ZygiskModuleImpl };
    if let Some(handler) = unsafe { (*impl_ptr).pre_app_specialize } {
        unsafe { handler(api_handle) };
    }
}
```

- [ ] **步骤 2：重写 `src/module.rs`——正确入口签名 + 生命周期**

```rust
type ModuleEntry = unsafe extern "C" fn(api: *mut ZygiskModuleApi, env: *mut c_void) -> i32;

pub struct ZygiskModule {
    pub handle: *mut c_void,
    pub api_handle: *mut c_void,
    pub id: alloc::string::String,
}

impl ZygiskModule {
    pub fn load(fd: i32) -> Option<Self> {
        // 通过 android_dlopen_ext 或 dlopen 加载 zygisk.so
        // dlsym("zygisk_module_entry")
        // 创建 ModuleApiV4 实例 + 填充
        // 创建 ZygiskModuleImpl 实例
        // api.base = &impl_table
        // 通过 get_global_jni_env() 获取 JNIEnv
        // entry(api_ptr, jni_env)
        // 返回 ZygiskModule
    }
}

pub fn load_modules(fds: &[i32]) -> alloc::vec::Vec<ZygiskModule> {
    fds.iter().filter_map(|&fd| ZygiskModule::load(fd)).collect()
}
```

- [ ] **步骤 3：集成到 `hook_context.rs`——`run_modules_pre/post`**

```rust
pub fn run_modules_pre(&mut self, fds: &[i32]) {
    self.modules = module::load_modules(fds);
    for m in &self.modules {
        module_api::call_pre_app_specialize(m.api_handle);
    }
}

pub fn run_modules_post(&mut self) {
    for m in &self.modules {
        // 通过 api→base→postAppSpecialize 调用
        module_api::call_post_app_specialize(m.api_handle);
    }
    if self.flags.has(Flags::DO_FUTILE_HIDE) {
        crate::solist::hide_modules();
    }
}
```

- [ ] **步骤 4：Commit**

```
git commit -m "Phase 7: 模块 API 表 + 回调表 + 入口签名修复 + 生命周期 dispatch"
```

---

### Phase 8 — JNI 代理函数

**目标：** `gen_jni_hooks.py` 生成 Rust 代码。RegisterNatives 钩子在**调 orig 前**替换方法指针。

- [ ] **步骤 1：定义 args 结构体**

```rust
#[repr(C)]
pub struct AppSpecializeArgsV5 {
    pub uid: *mut i32,
    pub gid: *mut i32,
    pub gids: *mut i32,
    pub runtime_flags: *mut i32,
    pub rlimits: *mut c_void,
    pub mount_external: *mut i32,
    pub se_info: *mut c_void,
    pub se_name: *mut c_void,
    pub NiceName: *mut c_void,
    pub managed_nice_name: *mut c_void,
    pub instruction_set: *mut c_void,   // jstring
    pub app_data_dir: *mut c_void,      // jstring
}

#[repr(C)]
pub struct ServerSpecializeArgsV1 {
    pub uid: *mut i32, pub gid: *mut i32, pub gids: *mut i32,
    pub runtime_flags: *mut i32, pub rlimits: *mut c_void,
    pub permitted_capabilities: *mut i64, pub effective_capabilities: *mut i64,
}
```

- [ ] **步骤 2：修改 `scripts/gen_jni_hooks.py`——生成三部分**

**1. JNI_METHOD_TABLE**：按 name + signature 匹配的查找表
```python
METHODS = [
    # (name, signature, handler_name, params_for_proxy)
    ("nativeForkAndSpecialize", "(III[III[IJJ)I", "nativeForkAndSpecialize_l",
     "uid, gid, gids, runtimeFlags, rlimits, permittedCapabilities, effectiveCapabilities"),
    ("nativeForkAndSpecialize", "(III[III[IJ[JJ)I", "nativeForkAndSpecialize_o",
     ...),
    # ... 全部 19 个
]
```

**2. `hook_and_save_zygote_methods`**：遍历 methods，匹配 name+signature，替换 fnPtr
```rust
pub unsafe fn hook_and_save_zygote_methods(
    env: *mut c_void,
    methods: *mut JNINativeMethod,
    n_methods: jint,
) {
    for i in 0..n_methods as isize {
        let m = &mut *methods.offset(i);
        let m_name = CStr::from_ptr(m.name).to_str().unwrap_or("");
        let m_sig = CStr::from_ptr(m.signature).to_str().unwrap_or("");
        for entry in JNI_METHOD_TABLE {
            if entry.name == m_name && entry.sig == m_sig {
                // 先保存原始指针（供代理函数调用）
                set_orig_fn_ptr(entry.name, entry.sig, m.fn_ptr);
                // 再替换为代理函数
                m.fn_ptr = entry.handler as *mut c_void;
                break;
            }
        }
    }
}
```

**3. 19 个 extern "C" 代理函数**
```rust
#[no_mangle]
pub unsafe extern "C" fn nativeForkAndSpecialize_l(
    env: *mut c_void, clazz: jclass,
    uid: jint, gid: jint, gids: *mut jint,
    runtime_flags: jint, rlimits: *mut c_void,
    permitted_capabilities: jlong, effective_capabilities: jlong,
) -> jint {
    let orig = match ORIG_FORK_AND_SPECIALIZE_L {
        Some(f) => core::mem::transmute::<_, unsafe extern "C" fn(
            *mut c_void, jclass, jint, jint, *mut jint, jint, *mut c_void, jlong, jlong) -> jint>(f),
        None => return -1,
    };
    let mut args = AppSpecializeArgsV5 { ... };
    let mut ctx = HookContext::new(env, (&raw mut args).cast(), "com.android.internal.os.Zygote");
    ctx.native_fork_and_specialize_pre();
    let pid = orig(env, clazz, uid, gid, gids, runtime_flags, rlimits,
                   permitted_capabilities, effective_capabilities);
    if pid == 0 {
        // 子进程：native_specialize_app_process_pre/post 会处理
    } else {
        ctx.native_fork_and_specialize_post();
    }
    pid
}
```

- [ ] **步骤 3：修改 `jni_env.rs` 的 `hook_register_natives`——ART 前替换**

```rust
pub unsafe extern "C" fn hook_register_natives(
    env: *mut c_void, clazz: jclass,
    methods: *mut JNINativeMethod, n_methods: jint,
) -> jint {
    let orig = ORIG_REGISTER_NATIVES.load(Ordering::Relaxed);
    if orig.is_null() { return 0; }
    let orig_fn: unsafe extern "C" fn(*mut c_void, jclass, *mut JNINativeMethod, jint) -> jint =
        core::mem::transmute(orig);

    // 在调 orig 之前替换指针（ART 在 orig 中读取 fnPtr）
    let class_name = get_class_name(env, clazz);
    if class_name.as_deref() == Some("com.android.internal.os.Zygote") {
        proxy_gen::hook_and_save_zygote_methods(env, methods, n_methods);
    }

    let ret = orig_fn(env, clazz, methods, n_methods);
    ret
}
```

- [ ] **步骤 4：运行脚本 + 检查到 repo**

```bash
python3 scripts/gen_jni_hooks.py > native/src/zygisk_inject/src/proxy_gen.rs
```

- [ ] **步骤 5：CI 构建**

（注：Phase 8 依赖 Phase 5 的 HookContext、Phase 7 的模块类型、Phase 6 的 IPC——但这些已有符号存根，编译通过。运行时行为在 Phase 9 完善）

- [ ] **步骤 6：Commit**

```
git commit -m "Phase 8: JNI 代理函数——19 Rust extern C + ART 前替换 + 签名匹配"
```

---

### Phase 9 — 进程生命周期完整集成

**目标：** 填充 HookContext 所有生命周期方法的真实逻辑。

- [ ] **步骤 1：`fork_pre` + `fork_post` 完整实现**

```rust
pub fn fork_pre(&mut self) {
    let mut set: libc::sigset_t = unsafe { core::mem::zeroed() };
    unsafe { libc::sigemptyset(&mut set) };
    unsafe { libc::sigaddset(&mut set, libc::SIGCHLD) };
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, core::ptr::null_mut()) };
    fd::record_open_fds(&mut self.allowed_fds);
    for fd in 0..3 { self.allowed_fds.add(fd); }
}
pub fn fork_post(&mut self) {
    let mut set: libc::sigset_t = unsafe { core::mem::zeroed() };
    unsafe { libc::sigemptyset(&mut set) };
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &set, core::ptr::null_mut()) };
}
```

- [ ] **步骤 2：`app_specialize_pre/post` 完整实现**

`pre`：IPC remote_get_info → 设置 flags → 模块加载
`post`（父进程侧）：sanitize_fds

- [ ] **步骤 3：`native_specialize_app_process_pre/post` 完整实现（子进程路径）**

`pre`（子进程）：获取 Forked context → IPC + flags + 模块加载
`post`（子进程）：模块 post → SoList 隐藏 → ZYGISK_ENABLED env → FD 清理

- [ ] **步骤 4：`native_fork_system_server_pre/post` 完整实现**

`pre`：IPC 获取 info + 系统服务器模块 → bits 回传
`post`：模块 post

- [ ] **步骤 5：CI 构建 + 部署验证**

- [ ] **步骤 6：Commit**

```
git commit -m "Phase 9: 进程生命周期完整集成——父子进程路径分离"
```

---

### Phase 10 — unshare + 安全/清理逻辑

- [ ] **步骤 1：`new_unshare` 完整 denylist/sulist 逻辑**

```rust
extern "C" fn new_unshare(flags: i32) -> i32 {
    let f: UnshareFn = match orig_fn(HookSlot::Unshare) { Some(f) => f, None => return -1 };
    let res = unsafe { f(flags) };
    if res == 0 && (flags & libc::CLONE_NEWNS) != 0 {
        if let Some(ctx) = hook_context::current_ctx() {
            if ctx.flags.has(Flags::DO_ALLOW) {
                ipc::request_sulist();
            } else if !ctx.flags.has(Flags::ALLOWLIST_ENFORCED)
                && ctx.flags.has(Flags::DO_REVERT_UNMOUNT)
            {
                ipc::request_umount();
            }
            unsafe { f(libc::CLONE_NEWNS) };
            if ctx.flags.has(Flags::RESTORE_MOUNT_EXTERNAL_NONE) {
                unsafe { *((*ctx.args).mount_external) = 0; }
            }
        }
    }
    res
}
```

- [ ] **步骤 2：logd 管理 + FD 清理集成**

- [ ] **步骤 3：Commit**

```
git commit -m "Phase 10: unshare denylist + logd + FD 清理"
```

---

### Phase 11 — 自卸载

**文件：** `src/unload.rs` + `src/hooks.rs`

- [ ] **步骤 1：`unhook_functions`——遍历 PLT_HOOK_LIST 恢复**

```rust
pub static SHOULD_UNLOAD: AtomicBool = AtomicBool::new(false);
pub static SELF_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

pub fn unhook_functions() -> bool {
    // 遍历 PLT_HOOK_LIST，每个注册原始函数重新通过 lsplt RegisterHook
    // 然后 CommitHook
    true
}
```

- [ ] **步骤 2：musttail asm stub**

```rust
#[cfg(target_arch = "aarch64")]
#[naked]
pub unsafe extern "C" fn dlclose_self(handle: *mut c_void) -> ! {
    core::arch::naked_asm!(
        "mov x0, {handle}",
        "b dlclose",
        handle = in(reg) handle,
    );
}
```

- [ ] **步骤 3：`pthread_attr_destroy` hook**

```rust
extern "C" fn new_pthread_attr_destroy(attr: *mut c_void) -> i32 {
    let f: PthreadAttrDestroyFn = match orig_fn(HookSlot::PthreadAttrDestroy) { Some(f) => f, None => return -1 };
    let ret = unsafe { f(attr) };
    if SHOULD_UNLOAD.load(Ordering::Acquire) {
        unhook_functions();
        unsafe { dlclose_self(SELF_HANDLE.load(Ordering::Relaxed)) }
    }
    ret
}
```

- [ ] **步骤 4：集成到 `HookContext::Drop`**

```rust
impl Drop for HookContext {
    fn drop(&mut self) {
        CURRENT_CTX.store(core::ptr::null_mut(), Ordering::Release);
        if !self.env.is_null() {
            crate::jni_env::restore_jni_env(self.env);
        }
        for m in &self.modules {
            // 清零模块 API（框架侧指针置零）
        }
        SHOULD_UNLOAD.store(true, Ordering::Release);
    }
}
```

- [ ] **步骤 5：Commit**

```
git commit -m "Phase 11: 自卸载——unhook + naked_asm! + pthread_attr_destroy"
```

---

### Phase 12 — SoList 隐藏 + memfd 覆盖

**文件：** `src/solist.rs`

- [ ] **步骤 1：soinfo 链表遍历 + NullifySoName**

```rust
pub fn initialize() -> bool {
    // dlsym("__solist") → 遍历 soinfo 链表
    // 或降级到 ELF 符号表扫描
}
pub fn nullify_so_name(fragment: &[u8]) -> bool;
```

- [ ] **步骤 2：memfd mremap 覆盖**

```rust
pub fn hide_memfd_mappings() {
    // 扫描 /proc/self/maps
    // 对 "/memfd:jit-zygisk-cache" 和 "/modules/" 映射：
    //   1. mmap 匿名页（相同大小，PROT_WRITE）
    //   2. memcpy(匿名页, 原始地址, size) 保留内容
    //   3. mremap(匿名页, size, MREMAP_MAYMOVE|MREMAP_FIXED, 原始地址)
    //   4. mprotect(原始地址, size, 原始权限)
}
pub fn hide_modules() { initialize(); nullify_so_name(...); hide_memfd_mappings(); }
```

- [ ] **步骤 3：集成到 `run_modules_post`**

- [ ] **步骤 4：Commit**

```
git commit -m "Phase 12: SoList 隐藏 + memfd mremap 覆盖"
```

---

## 执行与验证策略

### CI 每次提交验证
1. 编译 4 架构 .so
2. `readelf -d` 确认 0 `.init_array`
3. `readelf --dyn-syms` 确认所有符号导出

### VPhoneOS 部署验证
每阶段部署：
1. 开启 Zygisk → 重启
2. 确认无黑屏（system_server 不崩溃）
3. 检查 magisk.log 和 kernel log

### 已知风险

| 风险 | 等级 | 缓解 |
|------|------|------|
| `dlsym("JNI_RegisterNatives")` 在某些 Android 版本不存在 | 🟧 | dlsym 一次 → 遍历比较。失败回退到预计算偏移表 |
| JavaVM vtable GetEnv 偏移跨版本不同 | 🟧 | 扫描 vtable 查找特征，或通过已知偏移（标准 JNI 规范保证偏移稳定） |
| `#[naked]` + `naked_asm!` 编译器版本差异 | 🟡 | CI 锁定 nightly 版本 |
| `__solist` 在 Android 12+ 不可见 | 🟧 | 降级到 ELF 符号表扫描 / 已知偏移 |
| no_std 下无 spin crate | 🟡 | Atomic + UnsafeCell 手写 Once |
| mmap-per-allocation 碎片化 | 🟡 | 必要时加 bump 分配器层 |
| Daemon socket 路径 | 🟧 | get_magisk_tmp 解析 /proc/self/mountinfo |
| 32-/64-bit JNINativeInterface 差异 | 🟡 | cfg(target_pointer_width) |
| Samsung/GrapheneOS JNI 签名变体 | 🟡 | 在 JNI_METHOD_TABLE 中追加条目 |
| `*mut JNINativeMethod` 转换的 UB 风险 | 🟧 | 与 C++ 版一致的不安全假设，通过指针转换文档化 |
