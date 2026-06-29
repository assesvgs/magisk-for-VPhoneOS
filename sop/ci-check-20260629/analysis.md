# CI 错误分析报告

## 运行信息

- **Run ID**: 28359999604
- **Branch**: kitsune-mask-integration
- **创建时间**: 2026-06-29T08:49:34Z
- **Display Title**: F2: kitsune.cpp — update_deny_flags 签名与 CXX bridge 一致（rust::Str, uint…

### 失败平台与 Job

| Job | 结论 | Job DatabaseId |
|---|---|---|
| Test building on ubuntu-24.04 | failure | 84012093564 |
| Test building on windows-2025 | failure | 84012093632 |
| Build Magisk artifacts | failure | 84012093647 |

- ubuntu-24.04 和 windows-2025 报告完全相同的 **C++ 编译错误**（约 60+ 个 error），分为 `main.cpp`、`entry.cpp`、`hook.cpp`、`memory.cpp` 四个编译单元
- Build Magisk artifacts 报告 **1 个 Rust 编译错误**（E0432）和 2 个 warning

---

## 错误分类统计

### C++ 错误（影响: ubuntu-24.04, windows-2025）

| # | 错误概要 | 文件 | 出现次数(每平台) | 严重等级 |
|---|---------|------|:---:|:--------:|
| C1 | `fatal error: 'selinux.hpp' file not found` | `src/core/zygisk/main.cpp:9` | 1 | **CRITICAL** |
| C2 | `redefinition of 'ZygiskRequest' as different kind of symbol` (namespace vs enum class) | `src/core/zygisk/zygisk.hpp:9` | 2 (entry.cpp, hook.cpp) | **CRITICAL** |
| C3 | `no matching function for call to 'connect_daemon'` — 签名不匹配 (1 arg vs 2 args) | `src/core/zygisk/zygisk.hpp:42` | 2 | **CRITICAL** |
| C4 | `no member named 'GET_INFO' in 'ZygiskRequest'` | `entry.cpp:35` | 1 | HIGH |
| C5 | `no viable overloaded '='` — `rust::Vec<int32_t>` 无法赋值给 `std::vector<int>` | `entry.cpp:41` | 1 | HIGH |
| C6 | `no member named 'SULIST_ROOT_NS' in 'ZygiskRequest'` | `entry.cpp:49` | 1 | HIGH |
| C7 | `no member named 'REVERT_UNMOUNT' in 'ZygiskRequest'` | `entry.cpp:58` | 1 | HIGH |
| C8 | `unknown type name 'module_info'; did you mean 'ModuleInfo'?` | `entry.cpp:81,85` | 2 | HIGH |
| C9 | `use of undeclared identifier 'module_list'` | `entry.cpp:80,84` | 4 (2 lines × 2 platforms) | HIGH |
| C10 | `use of undeclared identifier 'xreadlink'; did you mean 'readlink'?` | `entry.cpp:94` | 1 | MEDIUM |
| C11 | `unknown type name 'pollfd'` | `entry.cpp:106` | 1 | MEDIUM |
| C12 | `use of undeclared identifier 'send_fds'` | `entry.cpp:130` | 1 | MEDIUM |
| C13 | `unknown type name 'sock_cred'` | `entry.cpp:143` | 1 | MEDIUM |
| C14 | `use of undeclared identifier 'check_pkg_refresh'` | `entry.cpp:149` | 1 | MEDIUM |
| C15 | `use of undeclared identifier 'sulist_enabled'` | `entry.cpp:151` | 1 | MEDIUM |
| C16 | `use of undeclared identifier 'get_manager'` | `entry.cpp:153` | 1 | MEDIUM |
| C17 | `no template named 'stateless_allocator'` | `memory.hpp:21` | 2 (hook.cpp, memory.cpp) | **CRITICAL** |
| C18 | `use of undeclared identifier 'allocator'` (应为类型而非 `memory_block::allocate`) | `memory.hpp:23,30,36` | 6 | **CRITICAL** |
| C19 | `template argument for template type parameter must be a type` (从 C18 级联) | `memory.hpp:23,30,36` | 6 | **CRITICAL** |
| C20 | `no member named 'string' in namespace 'jni_hook'` | `memory.hpp:43` | 2 | **CRITICAL** |
| C21 | `no member named 'hash_map' in namespace 'jni_hook'` | `hook.cpp:26` | 1 | **CRITICAL** |
| C22 | `no member named 'tree_map' in namespace 'jni_hook'; did you mean 'phmap::btree_map'?` | `hook.cpp:27` | 1 | **CRITICAL** |
| C23 | `no type named 'string' in namespace 'jni_hook'; did you mean simply 'string'?` | `hook.cpp:28` | 1 | **CRITICAL** |
| C24 | `use of undeclared identifier 'xstring'` (从 C23 级联) | `hook.cpp:61` | 3 | MEDIUM |
| C25 | `use of undeclared identifier 'tree_map'` (从 C22 级联) | `hook.cpp:61` | 2 | MEDIUM |
| C26 | `no template named 'hash_map'` (从 C21 级联) | `hook.cpp:61` | 1 | MEDIUM |
| C27 | `use of undeclared identifier 'xmmap'; did you mean 'mmap'?` | `memory.cpp:20` | 1 | MEDIUM |
| C28 | `use of undeclared identifier 'align_to'` | `memory.cpp:24` | 1 | MEDIUM |

### Rust 错误（影响: Build Magisk artifacts）

| # | 错误概要 | 文件 | 出现次数 | 严重等级 |
|---|---------|------|:-------:|:--------:|
| R1 | `error[E0432]: unresolved import 'base::libc::user_regs_struct'` — no `user_regs_struct` in root | `core/zygisk/ptrace_inject.rs:111` | 1 | **CRITICAL** |

### Warnings（不影响编译结果，但需注意）

| Warning | 文件 | 行 |
|---------|------|:--:|
| `unused function 'system_lnk'` | `core/deny/revert.cpp:58` | 1 |
| `unused function 'lazy_unmount'` | `core/deny/revert.cpp:126` | 1 |
| `unused import: 'set_prop'` | `core/bootstages.rs:17` | 1 |
| `unused import: 'std::os::fd::FromRawFd'` | `core/magisk.rs:12` | 1 |

---

## 每个错误详细分析

### C1: `fatal error: 'selinux.hpp' file not found` — `src/core/zygisk/main.cpp:9`

- **原始错误**:
  ```
  src/core/zygisk/main.cpp:9:10: fatal error: 'selinux.hpp' file not found
      9 | #include <selinux.hpp>
        |          ^~~~~~~~~~~~~
  ```
- **根因分析**: `main.cpp` 使用了 `#include <selinux.hpp>`，但该文件不在 include path 中或已在重构中被移除/重命名。
- **修复建议**: 确认 `selinux.hpp` 是否应存在于 `src/core/include/` 下；如果该文件已被 Rust 侧取代，需修复 `main.cpp` 的 include。

---

### C2: `redefinition of 'ZygiskRequest'` — `src/core/zygisk/zygisk.hpp:9`

- **原始错误**:
  ```
  src/core/zygisk/zygisk.hpp:9:11: error: redefinition of 'ZygiskRequest' as different kind of symbol
      9 | namespace ZygiskRequest {
        |           ^
  src/core/include/../core-rs.hpp:55:12: note: previous definition is here
     55 | enum class ZygiskRequest : ::std::int32_t {
        |            ^
  ```
- **根因分析**: `zygisk.hpp` 将 `ZygiskRequest` 声明为 `namespace`，但 Rust CXX bridge 生成的 `core-rs.hpp` 已经将其定义为 `enum class`。这是 kitsune-mask-integration merge 时引入的冲突：一边是旧 C++ 代码的 namespace 风格，一边是新 Rust CXX bridge 的 enum class 风格。
- **修复建议**: 删除 `zygisk.hpp` 中的 `namespace ZygiskRequest { ... }` 定义，改用 `core-rs.hpp` 中的 `enum class ZygiskRequest`。所有引用 `ZygiskRequest::XXX` 的代码必须使用 enum class 语法。

---

### C3: `no matching function for call to 'connect_daemon'` — `src/core/zygisk/zygisk.hpp:42`

- **原始错误**:
  ```
  src/core/zygisk/zygisk.hpp:42:14: error: no matching function for call to 'connect_daemon'
     42 |     int fd = connect_daemon(+RequestCode::ZYGISK);
        |              ^~~~~~~~~~~~~~
  src/core/include/core.hpp:23:12: note: candidate function not viable: no known conversion from
      'std::enable_if_t<std::is_enum<RequestCode>::value, std::underlying_type_t<RequestCode>>' (aka 'int')
      to 'RequestCode' for 1st argument
     23 | inline int connect_daemon(RequestCode req) {
  src/core/include/../core-rs.hpp:196:39: note: candidate function not viable: requires 2 arguments, but 1 was provided
    196 | [[gnu::always_inline]] ::std::int32_t connect_daemon(::RequestCode code, bool create) noexcept;
  ```
- **根因分析**: `connect_daemon` 有两个重载：C++ 版本 (1 个参数 `RequestCode`) 和 Rust CXX bridge 版本 (2 个参数 `RequestCode code, bool create`)。`zygisk.hpp` 中调用 `+RequestCode::ZYGISK` 将枚举转为 int，与两个重载都不匹配。同时 Rust 版要求第二参数 `create`。
- **修复建议**: 统一使用 Rust CXX bridge 的签名：`connect_daemon(RequestCode::ZYGISK, false)`，或修改 C++ 重载使其兼容。

---

### C4: `no member named 'GET_INFO' in 'ZygiskRequest'` — `entry.cpp:35`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:35:48: error: no member named 'GET_INFO' in 'ZygiskRequest'
     35 |     if (int fd = zygisk_request(ZygiskRequest::GET_INFO); fd >= 0) {
        |                                 ~~~~~~~~~~~~~~~^
  ```
- **根因分析**: C2 的错误导致 `ZygiskRequest` 被当作 namespace 而不是 enum class。即使修复 C2，也需要确认 `GET_INFO` 在 `core-rs.hpp` 的 `enum class ZygiskRequest` 中是否被正确定义（可能命名不同，或用 Rust 的 CXX bridge 导出方式不同，可能是短名称）。
- **修复建议**: 检查 `core-rs.hpp` 中 `enum class ZygiskRequest` 的枚举值列表，将 `GET_INFO` 替换为实际存在的枚举项名称。

---

### C5: `no viable overloaded '='` (rust::Vec vs std::vector) — `entry.cpp:41`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:41:17: error: no viable overloaded '='
     41 |             fds = recv_fds(fd);
        |             ~~~ ^ ~~~~~~~~~~~~
  ```
  尝试将 `::rust::Vec<::std::int32_t>` 赋值给 `std::vector<int>`。
- **根因分析**: `recv_fds` 返回 Rust 侧的 `Vec<int32_t>` (CXX bridge 生成类型)，但 `fds` 被声明为 `std::vector<int>`。CXX bridge 不会自动在两种 Vec 类型之间转换。
- **修复建议**: 将 `fds` 的类型改为 `::rust::Vec<::std::int32_t>`，或在赋值前执行手动转换。

---

### C6: `no member named 'SULIST_ROOT_NS' in 'ZygiskRequest'` — `entry.cpp:49`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:49:48: error: no member named 'SULIST_ROOT_NS' in 'ZygiskRequest'
     49 |     if (int fd = zygisk_request(ZygiskRequest::SULIST_ROOT_NS); fd >= 0) {
  ```
- **根因分析**: 同 C4，枚举值不存在于 Rust CXX bridge 版本中。
- **修复建议**: 检查 `core-rs.hpp` 中枚举值名称，使用正确名称。

---

### C7: `no member named 'REVERT_UNMOUNT' in 'ZygiskRequest'` — `entry.cpp:58`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:58:48: error: no member named 'REVERT_UNMOUNT' in 'ZygiskRequest'
     58 |     if (int fd = zygisk_request(ZygiskRequest::REVERT_UNMOUNT); fd >= 0) {
  ```
- **根因分析**: 同 C4、C6。
- **修复建议**: 检查 `core-rs.hpp` 中枚举值名称，使用正确名称或添加缺失的枚举值。

---

### C8: `unknown type name 'module_info'; did you mean 'ModuleInfo'?` — `entry.cpp:81,85`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:81:22: error: unknown type name 'module_info'; did you mean 'ModuleInfo'?
     81 |             [](const module_info &info) { ... });
        |                      ^~~~~~~~~~~
        |                      ModuleInfo
  src/core/include/core.hpp:30:8: note: 'ModuleInfo' declared here
     30 | struct ModuleInfo;
  ```
- **根因分析**: 代码使用小写 `module_info`，但 Rust CXX bridge (或核心头文件) 中定义的是大写 `ModuleInfo`。
- **修复建议**: 将 `module_info` 替换为 `ModuleInfo`。

---

### C9: `use of undeclared identifier 'module_list'` — `entry.cpp:80,84`

- **原始错误**:
  ```
  src/core/zygisk/entry.cpp:80:24: error: use of undeclared identifier 'module_list'
     80 |         std::transform(module_list->begin(), module_list->end(), ...);
  ```
- **根因分析**: `module_list` 在 `zygisk.hpp` 的旧 namespace 中被声明，但由于 C2 冲突，整个 namespace 被破坏，`module_list` 无法解析。或者在 Rust CXX bridge 重构后 `module_list` 被移到了其他位置。
- **修复建议**: 检查 Rust CXX bridge 中 module_list 的导出方式，在 entry.cpp 中引入适当的 include 或 extern 声明。

---

### C10–C16: 各种 `undeclared identifier` — `entry.cpp`

具体包括:
- **C10** `xreadlink` (line 94): 可能来自 `kitsune.hpp`，未被包含
- **C11** `pollfd` (line 106): 缺少 `#include <poll.h>`
- **C12** `send_fds` (line 130): 工具函数，可能来自 `kitsune.hpp`
- **C13** `sock_cred` (line 143): 类型，可能来自 `selinux.hpp` 或 `kitsune.hpp`
- **C14** `check_pkg_refresh` (line 149): 工具函数
- **C15** `sulist_enabled` (line 151): 全局变量
- **C16** `get_manager` (line 153): 工具函数

- **根因分析**: 这些符号大多由 `kitsune.hpp` 提供（或由 selinux.hpp / core.hpp 等提供）。由于 `selinux.hpp` 缺失（C1）和 `zygisk.hpp` 被破坏（C2），相关头文件的 include 链断裂导致所有这些符号无法解析。
- **修复建议**: 首要修复 C1 和 C2，然后大多数符号错误将自动消失。对于 `pollfd`，显式添加 `#include <poll.h>`。

---

### C17: `no template named 'stateless_allocator'` — `memory.hpp:21`

- **原始错误**:
  ```
  src/core/zygisk/memory.hpp:21:19: error: no template named 'stateless_allocator'
     21 | using allocator = stateless_allocator<T, memory_block>;
        |                   ^
  ```
- **根因分析**: `stateless_allocator` 模板未找到。这是一个自定义分配器适配器，可能定义在 `kitsune.hpp` 或某个未被包含的头文件中。
- **修复建议**: 添加包含 `stateless_allocator` 定义的头文件，或使用 `std::allocator` 替代。

---

### C18–C19: `use of undeclared identifier 'allocator'` / `template argument for template type parameter must be a type` — `memory.hpp:23,30,36`

- **原始错误**: C17 导致 `allocator` 别名未定义，因此 `allocator<char>` 等使用处全部级联失败。
- **根因分析**: 从 C17 级联而来。
- **修复建议**: 修复 C17 即可。

---

### C20: `no member named 'string' in namespace 'jni_hook'` — `memory.hpp:43`

- **原始错误**:
  ```
  src/core/zygisk/memory.hpp:43:37: error: no member named 'string' in namespace 'jni_hook'
     43 | template <> struct HashEq<jni_hook::string> : StringHashEqT<char> {};
  ```
- **根因分析**: `jni_hook::string` 已被移除或重命名。同 C21–C23。
- **修复建议**: 使用 `std::string` 替换，或添加 `using string = std::string;` 到 `jni_hook` 命名空间。

---

### C21–C23: `jni_hook` 命名空间成员缺失 — `hook.cpp:26,27,28`

- **原始错误**:
  - C21: `no member named 'hash_map' in namespace 'jni_hook'`
  - C22: `no member named 'tree_map' in namespace 'jni_hook'; did you mean 'phmap::btree_map'?`
  - C23: `no type named 'string' in namespace 'jni_hook'; did you mean simply 'string'?`
- **根因分析**: 在 Rust CXX bridge 重构中，`jni_hook` 命名空间下的类型别名(`hash_map`, `tree_map`, `string`)被移除或移动到其他位置。这些类型原本由 CXX bridge 生成的头文件提供，重构后未更新引用它们的 C++ 代码。
- **修复建议**: `hash_map` → `phmap::flat_hash_map`，`tree_map` → `phmap::btree_map`，`string` → `std::string`。

---

### C24–C26: 从 C21–C23 级联的错误 — `hook.cpp:61`

- **根因分析**: `xstring` 别名和 `tree_map`、`hash_map` 的缺失导致 `jni_method_map` 声明完全失败。修复 C21–C23 即可。

---

### C27: `use of undeclared identifier 'xmmap'; did you mean 'mmap'?` — `memory.cpp:20`

- **原始错误**:
  ```
  src/core/zygisk/memory.cpp:20:40: error: use of undeclared identifier 'xmmap'; did you mean 'mmap'?
     20 |         _area = static_cast<uint8_t *>(xmmap(...));
  ```
- **根因分析**: `xmmap` 是 `kitsune.hpp` 提供的包装函数，未包含该头文件。
- **修复建议**: 添加 `#include` 或直接使用 `mmap`。

---

### C28: `use of undeclared identifier 'align_to'` — `memory.cpp:24`

- **原始错误**:
  ```
  src/core/zygisk/memory.cpp:24:28: error: use of undeclared identifier 'align_to'
     24 |     return _curr.fetch_add(align_to(sz, ALIGN));
  ```
- **根因分析**: `align_to` 是 `kitsune.hpp` 提供的工具函数，未包含该头文件。
- **修复建议**: 添加 `#include`，或使用 `(sz + ALIGN - 1) & ~(ALIGN - 1)` 手动对齐。

---

### R1: `error[E0432]: unresolved import 'base::libc::user_regs_struct'` — `core/zygisk/ptrace_inject.rs:111`

- **原始错误**:
  ```
  error[E0432]: unresolved import `base::libc::user_regs_struct`
     --> core/zygisk/ptrace_inject.rs:111:9
      |
  111 | pub use base::libc::user_regs_struct;
      |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `user_regs_struct` in the root
  ```
- **根因分析**: `base::libc` 模块不再导出 `user_regs_struct`。在较新版本的 Rust std / libc crate 中，此类型可能已被移动（例如到 `std::arch::x86_64` 或其他平台特定路径），或者当前 target 不匹配导致不可见。
- **修复建议**: 根据 `libc` crate 版本查找 `user_regs_struct` 的正确路径。如果仅在特定架构（如 x86_64）可用，需添加条件编译 `#[cfg(target_arch = "x86_64")]`。

---

## 跨平台关联分析

| 错误 | ubuntu-24.04 | windows-2025 | Build Magisk artifacts | 分析 |
|------|:---:|:---:|:---:|------|
| C1–C28 (C++ errors) | ✅ | ✅ | ❌ | **Ubuntu 和 Windows 完全相同的 C++ 错误集** — 与平台无关，是代码层面的问题 |
| R1 (Rust E0432) | ❌ | ❌ | ✅ | **仅影响 Build Magisk artifacts job** — 该 job 执行 Rust 编译（release），其他 job 的 Rust 部分可能被 sccache 缓存跳过 |

### 核心根因总结

所有 C++ 错误可以追溯到 **3 个根因**:

1. **根因 A — ZygiskRequest 冲突（C2 为首）**: `zygisk.hpp` 中的 `namespace ZygiskRequest` 与 `core-rs.hpp` 中的 `enum class ZygiskRequest` 冲突。这导致整个 `zygisk.hpp` 命名空间解析失败，级联引发 C4–C16 的符号找不到错误。修复此根因可消除约 60% 的 C++ 错误。

2. **根因 B — selinux.hpp 缺失（C1）**: `main.cpp` 的第 9 行 `#include <selinux.hpp>` 找不到文件。该头文件可能已从仓库中移除。修复后可使 `main.cpp` 编译通过。

3. **根因 C — jni_hook 命名空间重构（C17–C26）**: `memory.hpp` 和 `hook.cpp` 依赖 `jni_hook::` 下的 `stateless_allocator`、`hash_map`、`tree_map`、`string` 等类型别名，这些在 Rust CXX bridge 重构后不再可用。修复后可使 `memory.hpp` / `memory.cpp` / `hook.cpp` 编译通过。

### Rust 错误根因

4. **根因 R — user_regs_struct 路径变更（R1）**: `base::libc::user_regs_struct` 在当前 Rust 工具链或 libc crate 版本中不可用。需根据架构判断正确的导入路径或使用 C 头文件绑定。

### 修复优先级

| 优先级 | 错误 | 预估影响 |
|:------:|------|---------|
| P0 | R1 (Rust E0432) | 阻塞 Rust 编译，修复后可编译所有 Rust crate |
| P0 | C1 (selinux.hpp) | 阻塞 main.cpp 编译 |
| P0 | C2 (ZygiskRequest 冲突) | 阻塞 entry.cpp / hook.cpp，影响范围最大 |
| P0 | C17 (stateless_allocator) | 阻塞 memory.hpp / hook.cpp 编译 |
| P1 | C21–C23 (jni_hook 命名空间) | 阻塞 hook.cpp，需配合 C17 修复 |
| P2 | C10–C16 (entry.cpp 符号) | 大部分会随 C2 的修复而自动解决 |
