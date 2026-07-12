# Zygisk 功能缺口审查报告

> 对比 `ci-clean`（新 Rust no_std 实现）vs `kitsune-mask-integration`（原始 C++ 实现）
> 生成日期: 2026-07-12

## 1. 总览

| 维度 | 旧实现 | 新实现 |
|------|--------|--------|
| inject 库行数 | ~1948 行 | ~508 行 |
| 核心生命周期 | HookContext 完整实现 | **完全缺失** |
| JNI 代理函数 | 19 个版本（Android L~14、Samsung、GrapheneOS） | **0 个** |
| 模块 API | v1/v2/v4 完整实现 | **仅 dlopen，无 API 表填充** |
| 自卸载机制 | pthread_attr_destroy + unhook_functions + dlclose(musttail) | **完全缺失** |
| 功能覆盖率估算 | 100%（基线） | **约 15-20%** |

---

## 2. PLT Hook 对比

| 符号 | 旧回调行为 | 新回调行为 | 缺口 |
|------|-----------|-----------|------|
| **fork** | 返回 g_ctx->pid 缓存（绕过实际 fork） | 直接调原函数 | pid 缓存逻辑 |
| **unshare** | CLONE_NEWNS 后: 请求 sulist/unmount → 二次 unshare → 恢复 mount_external → 日志静音 | 直接调原函数 | **全部**：denylist/unmount/mount_external 逻辑 |
| **selinux_android_setcontext** | zygisk_get_logd() 预拉取 logd fd | 直接调原函数 | logd 预拉取 |
| **android_log_close** | 检查 SKIP_CLOSE_LOG_PIPE 标志 | 直接调原函数 | 条件性关闭 |
| **androidSetCreateThreadFunc** | 触发 hook_jni_env() | **未 hook** | 失去 JNI hook 触发时机 |
| **pthread_attr_destroy** | 自卸载: unhook_functions + dlclose(musttail) | **未 hook** | inject so 永远不卸载 |
| **strdup** | 旧无此 hook | **新实现独有**，检测 "ZygoteInit" | 新的触发方式 |
| **dlclose** | 旧无此 hook | **新实现独有**，/libnativebridge.so | 新的 native bridge 支持 |

---

## 3. 进程生命周期管理（HookContext 完整缺失）

### 3.1 HookContext 成员

| 成员 | 旧 hook.cpp | 新实现 |
|------|------------|--------|
| JNIEnv *env | ✅ line 76 | ❌ |
| args union（app/server） | ✅ line 77-81 | ❌ |
| const char *process | ✅ line 83 | ❌ |
| int pid | ✅ line 86 | ❌ |
| flags bitset（12 标志位） | ✅ line 87 | ❌ |
| uint32_t info_flags | ✅ line 88 | ❌ |
| bitset allowed_fds | ✅ line 89 | ❌ |
| vector exempted_fds | ✅ line 90 | ❌ |
| list modules | ✅ line 84 | ❌ |
| vector register_info / ignore_info | ✅ line 92-106 | ❌ |
| mutex hook_info_lock | ✅ line 104 | ❌ |

### 3.2 生命周期方法（全部缺失）

| 方法 | 旧行号 | 新实现 |
|------|--------|--------|
| fork_pre() | 428-454 | ❌ SIGCHLD 阻塞、获取 fd 列表 |
| fork_post() | 456-459 | ❌ SIGCHLD 解除阻塞 |
| app_specialize_pre() | 602-640 | ❌ IPC 获取 info、flags 设置、模块加载 |
| app_specialize_post() | 642-650 | ❌ 模块 post、ZYGISK_ENABLED env |
| server_specialize_pre() | 652-674 | ❌ 系统服务器模块加载、bitset 回传 |
| server_specialize_post() | 676-678 | ❌ 模块 post |
| nativeForkAndSpecialize_pre/post() | 763-781 | ❌ JNI 钩子完整生命周期 |
| nativeSpecializeAppProcess_pre/post() | 731-742 | ❌ specialize 生命周期 |
| nativeForkSystemServer_pre/post() | 744-761 | ❌ 系统服务器 fork 生命周期 |
| sanitize_fds() | 461-512 | ❌ 关闭非白名单 fd |
| exempt_fd() | 720-727 | ❌ 模块 API |
| run_modules_pre() | 514-584 | ❌ 模块加载与初始化、memfd 隐藏 |
| run_modules_post() | 586-600 | ❌ 模块 post、SoList 隐藏 |
| ~HookContext() 析构 | 680-718 | ❌ JNI 恢复、API 清零、触发卸载 |

### 3.3 枚举与标志位（全部缺失）

| 枚举值 | 旧位置 | 说明 |
|--------|--------|------|
| POST_SPECIALIZE | hook.cpp:44 | post 阶段 |
| APP_FORK_AND_SPECIALIZE | hook.cpp:45 | fork+specialize 路径 |
| APP_SPECIALIZE | hook.cpp:46 | specialize 路径 |
| SERVER_FORK_AND_SPECIALIZE | hook.cpp:47 | 系统服务器路径 |
| DO_REVERT_UNMOUNT | hook.cpp:48 | denylist unmount |
| SKIP_CLOSE_LOG_PIPE | hook.cpp:49 | log 关闭条件 |
| DO_ALLOW | hook.cpp:50 | 白名单模式 |
| ALLOWLIST_ENFORCED | hook.cpp:51 | 白名单强制 |
| RESTORE_MOUNT_EXTERNAL_NONE | hook.cpp:52 | 挂载外部存储恢复 |
| DO_FUTILE_HIDE | hook.cpp:54 | SoList memfd 隐藏 |
| FLAG_MAX | hook.cpp:56 | 标志位总数 |

---

## 4. JNI Hook 层面

### 4.1 JNI 代理函数（19 个全部缺失）

旧实现 `gen_jni_hooks.py` 自动生成了以下代理函数，每个都：
1. 构造 AppSpecializeArgs_v5 / ServerSpecializeArgs_v1
2. 创建 HookContext（触发 restore_jni_env）
3. 调用 *_pre()
4. 调原始方法
5. 调用 *_post()
6. 返回 ctx.pid

| 函数 | 适配范围 | 新实现 |
|------|---------|--------|
| nativeForkAndSpecialize_l | Android 5-7 | ❌ |
| nativeForkAndSpecialize_o | Android 8 | ❌ |
| nativeForkAndSpecialize_p | Android 9 | ❌ |
| nativeForkAndSpecialize_q_alt | Android 10 | ❌ |
| nativeForkAndSpecialize_r | Android 11 | ❌ |
| nativeForkAndSpecialize_u | Android 14 | ❌ |
| nativeForkAndSpecialize_samsung_m | Samsung | ❌ |
| nativeForkAndSpecialize_samsung_n | Samsung | ❌ |
| nativeForkAndSpecialize_samsung_o | Samsung | ❌ |
| nativeForkAndSpecialize_samsung_p | Samsung | ❌ |
| nativeForkAndSpecialize_grapheneos_u | GrapheneOS | ❌ |
| nativeSpecializeAppProcess_q | Android 10 | ❌ |
| nativeSpecializeAppProcess_q_alt | Android 10 alt | ❌ |
| nativeSpecializeAppProcess_r | Android 11 | ❌ |
| nativeSpecializeAppProcess_u | Android 14 | ❌ |
| nativeSpecializeAppProcess_samsung_q | Samsung | ❌ |
| nativeSpecializeAppProcess_grapheneos_u | GrapheneOS | ❌ |
| nativeForkSystemServer_l | Android 5+ | ❌ |
| nativeForkSystemServer_samsung_q | Samsung | ❌ |

### 4.2 env_RegisterNatives 钩子

| 功能 | 旧实现 | 新实现 |
|------|--------|--------|
| 劫持所有 RegisterNatives 调用 | ✅ | ✅ |
| 通过 hookAndSaveJNIMethods 替换 JNI 方法 | ✅ | ❌ 仅捕获指针不替换 |
| jni_hook_list 保存原方法 | ✅ | ❌ |
| jni_method_map 保存 className→methodName→signature→fnPtr | ✅ | ❌ |
| 模块 API hookJniNativeMethods 支持 | ✅ | ❌ |

### 4.3 hook_jni_env / restore_jni_env

| 功能 | 旧实现 | 新实现 |
|------|--------|--------|
| libnativehelper.so fallback | ✅ 扫描 maps + dlopen | ❌ |
| new_functions 分配 | default_new（bump allocator） | mmap |
| mprotect 只读保护 | ❌（旧遗漏） | ✅ **改进** |
| restore_jni_env | ✅ 恢复 env->functions | ❌ **完全缺失** |

---

## 5. 模块加载与 API

### 5.1 模块加载

| 方面 | 旧实现 | 新实现 |
|------|--------|--------|
| 加载方式 | android_dlopen_ext fd-based | dlopen 文件系统路径 |
| 模块来源 | IPC 接收 fd（remote_get_info） | 直接扫描目录 |
| 加载时机 | run_modules_pre() 内 | load_modules() 存在但**不被调用** |
| 模块入口参数 | &api_table（API 表指针） | null（=segfault） |

### 5.2 API 表填充（关键缺口）

旧实现 `RegisterModuleImpl()` 填充：
```cpp
// v1
api->v1.hookJniNativeMethods = hookJniNativeMethods;
api->v1.pltHookRegister = [](dev,ino,sym,hook,orig) { ... };
api->v1.pltHookExclude = [](dev,ino,sym) { ... };
api->v1.pltHookCommit = []() { ... };
api->v1.connectCompanion = [](fd) { ... };
api->v1.setOption = [](opt) { ... };

// v2
api->v2.getModuleDir = [](id) { ... };
api->v2.getFlags = []() { ... };

// v4
api->v4.pltHookRegister = lsplt::RegisterHook;
api->v4.pltHookCommit = lsplt::CommitHook;
api->v4.exemptFd = [](fd) { ... };
```

新实现 `module.rs`：
```rust
let entry: ModuleEntry = unsafe { core::mem::transmute(entry_fn) };
let api = unsafe { entry(core::ptr::null_mut()) };  // 传 null!
```
- 传入 `null` → 模块崩溃
- 签名不匹配（Rust 声明 `fn(*mut c_void) -> *mut c_void`，实际是 `fn(api_table*, JNIEnv*) -> long`）
- API 表未填充 → 所有函数指针为 null

### 5.3 模块 API 函数（全部缺失）

| API | 旧位置 | 说明 |
|-----|--------|------|
| hookJniNativeMethods | hook.cpp:229-262 | 模块 hook Java 方法 |
| connectCompanion | hook.cpp:384-390 | 连接 companion 进程 |
| getModuleDir | hook.cpp:392-400 | 获取模块目录 fd |
| setOption | hook.cpp:402-413 | FORCE_DENYLIST_UNMOUNT / DLCLOSE_MODULE_LIBRARY |
| getFlags | hook.cpp:415-417 | 获取进程状态 |
| valid | hook.cpp:369-382 | 模块有效性验证 |
| tryUnload | module.hpp:210 | 热卸载 |
| clearApi | module.hpp:211 | API 指针清零 |
| pltHookRegister regex | hook.cpp:314-322 | 正则 PLT hook 注册 |
| pltHookExclude | hook.cpp:324-331 | 正则 PLT hook 排除 |
| plt_hook_process_regex | hook.cpp:333-355 | 正则匹配处理 |
| exemptFd | hook.cpp:720-727 | FD 豁免 |

---

## 6. 进程通信与 Daemon IPC

### 6.1 entry.cpp 中保留但未被调用的函数

| 函数 | 保留 | 被调用 |
|------|------|--------|
| remote_get_info() | ✅ | ❌（app_specialize_pre 缺失） |
| remote_request_sulist() | ✅ | ❌（unshare hook 不调用） |
| remote_request_umount() | ✅ | ❌（unshare hook 不调用） |
| connect_companion() | ✅ | ❌（模块 API 未填充） |

### 6.2 Daemon 侧差异

| 功能 | 旧 entry.cpp | 新 daemon.rs |
|------|-------------|-------------|
| system_server 模块 bitset 回传 | 创建 "unloaded" 文件 | 仅读取 _failed_ids，**不创建文件** |
| RevertUmount | get_clean_ns() 传路径字符串 | revert_unmount(pid)（架构不同，等价） |

---

## 7. 安全与清理

### 7.1 FD 管理（全部缺失）

| 功能 | 旧 | 新 |
|------|-----|-----|
| sanitize_fds() 关闭泄漏 fd | ✅ | ❌ |
| exempt_fd() 模块 API | ✅ | ❌ |
| fds_to_ignore 扩展 | ✅ | ❌ |
| zygisk_get_logd / zygisk_close_logd | ✅ | ❌ |
| fork_pre 记录打开 fd | ✅ | ❌ |

### 7.2 自卸载链（全部缺失）

完整链条：
```
HookContext::~HookContext()
  → should_unmap_zygisk = true
  → 遍历 jni_hook_list, RegisterNatives 恢复 JNI
  → delete jni_hook_list, operator delete(jni_method_map)
  → memory_block::release() 释放 4MB 分配区
  → 对每个模块 clearApi() 清零
  → hook_unloader()
    → PLT_HOOK_REGISTER(pthread_attr_destroy)
    → hook_commit()

VM 线程启动 → pthread_attr_destroy 被调用:
  → unhook_functions() → 注册原始函数恢复 PLT → lsplt::CommitHook()
  → dlclose(self_handle) with [[clang::musttail]]
```

**新实现：** 以上全部缺失。inject so 永远不卸载。

### 7.3 JNI 恢复

| 步骤 | 旧 | 新 |
|------|-----|-----|
| 构造函数调 restore_jni_env | ✅ | ❌ |
| 析构函数恢复 RegisterNatives | ✅ | ❌ |
| operator delete jni_method_map | ✅ | ❌ |
| memory_block::release() munmap 4MB | ✅ | ❌ |

---

## 8. 额外功能

### 8.1 memfd / SoList 隐藏

旧实现 `DO_FUTILE_HIDE` 时：
```
for /memfd:jit-zygisk-cache 和 /modules/ 映射:
  → mmap 匿名页 → memcpy → mremap 覆盖 → mprotect
→ SoList::NullifySoName() 清空 linker soinfo 的 realpath
```
**新实现：** `DO_FUTILE_HIDE` 从未设置，memfd 重映射缺失，SoList 从未调用。

### 8.2 gen_jni_hooks.py

自动生成 19 个 JNI 代理函数的脚本被移除。需重建代理函数生成逻辑。

### 8.3 memory 分配器架构差异

| 旧 | 新 |
|-----|-----|
| 4MB mmap bump allocator | mmap-per-allocation GlobalAlloc |
| memory_block::release() 一次性释放 | 每次 dealloc 独立 munmap |
| hash_map/tree_map/string 共享分配器 | Rust alloc Vec/String 独占 mmap |
| 与 jni_hook C++ 侧共享 | 不共享 |

功能等效，架构不同。

---

## 9. 全部缺口清单

### ❌ 关键缺失（Zygisk 完全不可用）

| # | 功能 | 源文件 | 说明 |
|---|------|--------|------|
| 1 | JNI 代理函数（19 个） | jni_hooks.hpp:6-351 | 无 nativeForkAndSpecialize/nativeSpecializeAppProcess/nativeForkSystemServer 代理 |
| 2 | HookContext 完整结构 | hook.cpp:75-141 | 进程生命周期管理完全不存在 |
| 3 | *_pre()/*_post() 方法（14 个） | hook.cpp:428-781 | 无 fork/app/server specialize 逻辑 |
| 4 | run_modules_pre/post | hook.cpp:514-600 | module.rs load_modules() 存在但不被调用 |
| 5 | API 表填充 | hook.cpp:272-312 | 传 null 给模块入口，模块立即崩溃 |
| 6 | androidSetCreateThreadFunc hook | hook.cpp:151-155 | 失去 JNI hook 触发时机 |
| 7 | pthread_attr_destroy hook（自卸载） | hook.cpp:204-223 | inject so 无法卸载 |
| 8 | unhook_functions | hook.cpp:854-868 | PLT hook 无法恢复 |
| 9 | hook_unloader | hook.cpp:838-852 | 卸载触发器不注册 |
| 10 | ~HookContext() 析构 | hook.cpp:680-718 | JNI 恢复、API 清零、卸载触发均缺失 |
| 11 | restore_jni_env | hook.cpp:941-945 | JNI env 表不恢复 |
| 12 | unshare 完整逻辑 | hook.cpp:163-181 | 无 denylist unmount/sulist mount/双 unshare |
| 13 | app_specialize_pre 内 denylist 决策 | hook.cpp:602-650 | 白名单/黑名单/allowlist 逻辑完全缺失 |
| 14 | env_RegisterNatives 替换而非仅捕获 | hook.cpp:885-892 | 只捕获指针不替换函数 |
| 15 | FD 清理 sanitize_fds | hook.cpp:461-512 | 子进程 fd 泄漏 |

### ⚠️ 重要缺失（模块 API 不可用）

| # | 功能 | 源文件 |
|---|------|--------|
| 16 | hookJniNativeMethods | hook.cpp:229-262 |
| 17 | connectCompanion | hook.cpp:384-390 |
| 18 | getModuleDir | hook.cpp:392-400 |
| 19 | setOption | hook.cpp:402-413 |
| 20 | getFlags | hook.cpp:415-417 |
| 21 | valid 模块验证 | hook.cpp:369-382 |
| 22 | pltHookRegister regex 版 | hook.cpp:314-322 |
| 23 | pltHookExclude | hook.cpp:324-331 |
| 24 | plt_hook_process_regex | hook.cpp:333-355 |
| 25 | exemptFd | hook.cpp:720-727 |
| 26 | fork_pre 记录 fd | hook.cpp:428-454 |
| 27 | selinux_android_setcontext logd 预拉取 | hook.cpp:184-189 |
| 28 | android_log_close 条件性关闭 | hook.cpp:192-199 |

### 🔧 次要缺失

| # | 功能 | 源文件 |
|---|------|--------|
| 29 | DO_FUTILE_HIDE memfd 重映射 | hook.cpp:535-566 |
| 30 | SoList::NullifySoName | solist.hpp:59-67 |
| 31 | SoList::Initialize | solist.hpp:69-104 |
| 32 | system_server 标记未加载模块 | entry.cpp:206-214 |
| 33 | zygisk_get_logd / zygisk_close_logd | hook.cpp 配套函数 |
| 34 | logging_muted = true | hook.cpp:177 |
| 35 | gen_jni_hooks.py 脚本 | gen_jni_hooks.py:1-310 |
| 36 | Samsung/GrapheneOS JNI 变体 | jni_hooks.hpp（各变体函数） |
| 37 | fork pid 缓存 | hook.cpp:158-160 |
| 38 | app_specialize_post ZYGISK_ENABLED env | hook.cpp:645 |
| 39 | app_specialize_post 释放 nice_name | hook.cpp:649 |
| 40 | fork_pre 阻塞/解除阻塞 SIGCHLD | hook.cpp:431,458 |

### ✅ 已保留或等效

| 功能 | 说明 |
|------|------|
| zygisk_inject_entry 入口 | ✅ |
| hook_functions() stub | ✅ entry.cpp:24 |
| lsplt PLT hook 基础设施 | ✅ cxx/plt_hook.cpp |
| JNI env 表替换 | ✅ cxx/jni_hook.cpp（但功能简化） |
| remote_get_info / remote_request_sulist / remote_request_umount | ✅ entry.cpp（但未被调用） |
| zygisk_request | ✅ zygisk.hpp:31-36 |
| connect_companion | ✅ entry.cpp（但未被调用） |
| get_process_info daemon 侧 | ✅ daemon.rs（大部分等价） |
| init_monitor ptrace 监控 | ✅ |
| trace_zygote ptrace 注入 | ✅ |
| #![no_std] 环境 | ✅ 新架构 |
| C++ stubs | ✅ stubs.cpp |
| Module load_modules() 目录扫描 | ✅ module.rs（但不被调用且 API 表未填充） |

---

> **估算：** 要达到功能对等，还需实现约 40 项独立功能（含 19 个 JNI 代理函数、HookContext 完整生命周期、模块 API 表填充、自卸载链、FD 管理、denylist 逻辑等）。
