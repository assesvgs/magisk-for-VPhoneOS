# 修复计划：32-bit zygote 注入 + 构建产物清理

> **面向 AI 代理的工作人员：** 必需子技能：使用 `subagent-driven-development`（推荐）或 `executing-plans` 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。
>
> **前置说明：** 本计划涉及三个独立问题，可以按任意顺序实施，无依赖关系。

**目标：** 修复 32-bit zygote 注入失败、消除 magisk64 与 magisk 的文件重复、消除 .init_array 中的 NULL 条目。

---

## 问题一：32-bit zygote 注入失败

### 根因

APK 中已经有 `lib/armeabi-v7a/libzygisk_inject.so`（32-bit），但部署时：
- `live_setup.sh` 只从 `lib/$ABI`（arm64-v8a）提取了所有文件，没有单独提取 `lib/$ABI32`（armeabi-v7a）中的 `libzygisk_inject.so`
- 所以 `/sbin/zygisk_inject` 只有 64-bit 版本
- 32-bit zygote `dlopen("/sbin/zygisk_inject")` 失败（64-bit .so 无法在 32-bit 进程中加载）

类似的问题之前也出现在 `magisk32` 上，已修复（`live_setup.sh:69` 有 `unzip -oj magisk.apk "lib/$ABI32/libmagisk32.so"`）。

### 修复

#### 文件 1：`scripts/live_setup.sh`

在已有的 `magisk32` 提取逻辑后面，增加 `zygisk_inject32` 提取：

```bash
# 提取 32-bit zygisk_inject（用于注入 32-bit zygote）
unzip -oj magisk.apk "lib/$ABI32/libzygisk_inject.so" 2>/dev/null || true
if [ -f libzygisk_inject.so ]; then
    mv libzygisk_inject.so zygisk_inject32
    chmod 755 zygisk_inject32
fi
```

此段加在 `magisk32` 处理代码块后面（约 line 71），在部署循环（line 134）之前。

**同时**需要在部署循环（line 134）中加入 `zygisk_inject32`：

```bash
# 旧：
for file in magisk magisk32 magisk64 magiskpolicy zygisk_inject stub.apk; do
# 新：
for file in magisk magisk32 magisk64 magiskpolicy zygisk_inject zygisk_inject32 stub.apk; do
```

否则提取出的 `zygisk_inject32` 不会被复制到 `DATABIN`，`bootstages.rs` 找不到它。

#### 文件 2：`native/src/core/zygisk/init_monitor.cpp`

修改 `inject_zygote()` 和 `find_zygote_by_polling()` 中的 `inject_lib` 路径选择：

```cpp
// 当前（错误）：对所有架构使用同一路径
auto inject_lib = string(get_magisk_tmp()) + "/zygisk_inject";

// 修复后：根据目标进程架构选择
string inject_lib;
if (program.find("32") != string::npos) {
    inject_lib = string(get_magisk_tmp()) + "/zygisk_inject32";
} else {
    inject_lib = string(get_magisk_tmp()) + "/zygisk_inject";
}
```

两个函数（`inject_zygote` 和 `find_zygote_by_polling`）都需做此修改。

#### 文件 3：`native/src/core/bootstages.rs`

现有代码结构（实际）：
```rust
// Zygisk 条件块（检查 ZygiskConfig 数据库设置）
if self.get_db_setting(DbEntryKey::ZygiskConfig) != 0 {
    // 检查 magisk32/64/zygisk_inject 是否存在并 warn
    let magisk32 = cstr!(concatcp!(DATABIN, "/magisk32"));
    if magisk32.exists() { ... }
    let tmp64 = buf.append_path(get_magisk_tmp()).append_path("magisk64");
    if !tmp64.exists() { warn!("...64-bit Zygisk unavailable"); }
    // 此处是 zygisk_inject 的 warn 检查
    let tmp_inject = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
    if !tmp_inject.exists() { warn!("zygisk: zygisk_inject not deployed, injection target missing"); }
}
// Zygisk 条件块外：复制文件到 /sbin/
let zygisk_inject = cstr!(concatcp!(DATABIN, "/zygisk_inject"));
if zygisk_inject.exists() {
    let tmp = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
    zygisk_inject.copy_to(tmp).log_ok();
}
```

修改为：

```rust
// Zygisk 条件块内：逐项检查
if self.get_db_setting(DbEntryKey::ZygiskConfig) != 0 {
    let magisk32 = cstr!(concatcp!(DATABIN, "/magisk32"));
    if magisk32.exists() { ... }
    let tmp64 = buf.append_path(get_magisk_tmp()).append_path("magisk64");
    if !tmp64.exists() { warn!("...64-bit Zygisk unavailable"); }

    // zygisk_inject 64-bit 存在性检查
    let tmp_inject = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
    if !tmp_inject.exists() { warn!("zygisk: zygisk_inject not deployed, 64-bit injection target missing"); }
    // zygisk_inject32 32-bit 存在性检查（新增）
    let tmp_inject32 = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject32");
    if !tmp_inject32.exists() { warn!("zygisk: zygisk_inject32 not deployed, 32-bit injection target missing"); }
}
// 条件块外：复制文件
let zygisk_inject = cstr!(concatcp!(DATABIN, "/zygisk_inject"));
if zygisk_inject.exists() {
    let tmp = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject");
    zygisk_inject.copy_to(tmp).log_ok();
}
// 新增：复制 32-bit zygisk_inject
let zygisk_inject32 = cstr!(concatcp!(DATABIN, "/zygisk_inject32"));
if zygisk_inject32.exists() {
    let tmp32 = buf.append_path(get_magisk_tmp()).append_path("zygisk_inject32");
    zygisk_inject32.copy_to(tmp32).log_ok();
}

---

## 问题二：magisk64 == magisk 文件重复

### 根因

`build.py:188-190` 中将编译好的 `magisk` 二进制复制为 `magisk32`/`magisk64`：

```python
# Rename magisk per architecture: magisk32 for 32-bit, magisk64 for 64-bit
magisk_file = out_dir / "magisk"
if magisk_file.exists():
    if arch in ("armeabi-v7a", "x86"):
        cp(magisk_file, out_dir / "magisk32")
    elif arch in ("arm64-v8a", "x86_64"):
        cp(magisk_file, out_dir / "magisk64")
```

对于 64-bit 架构（arm64-v8a, x86_64），`magisk64` 和 `magisk` 是完全相同的 64-bit 二进制。`Setup.kt` 将两者都打包进 APK，浪费约 470KB（arm64） + 510KB（x86_64）。

**注意：** 32-bit 架构的 `magisk32` **不是重复**——它确实是 32-bit 二进制（与 `magisk` 架构相同但作为独立文件存在）。不过 32-bit 架构上 `magisk` 和 `magisk32` 也是同一份文件，同样可以消除。

### 修复

#### 文件 4：`build.py`

移除 `collect_ndk_build()` 中的文件复制逻辑，改由部署脚本创建符号链接：

```python
def collect_ndk_build():
    for arch in build_abis.keys():
        arch_dir = Path("native", "libs", arch)
        out_dir = Path("native", "out", arch)
        for source in arch_dir.iterdir():
            target = out_dir / source.name
            mv(source, target)
        # magisk32/magisk64 不再在此处复制，由 live_setup.sh 在部署时创建符号链接
```

#### 文件 5：`app/buildSrc/src/main/java/Setup.kt`

修改 `archBins`：移除 `magisk64`（64-bit 架构）和 `magisk32`（32-bit 架构），因为部署时通过符号链接创建。

```kotlin
val archBins = mutableListOf("magiskboot", "magiskinit", "magiskpolicy", "libinit-ld.so")
if (abi in listOf("armeabi-v7a", "x86")) {
    archBins.add("magisk")
    // magisk32 由 live_setup.sh 创建为 magisk 的符号链接
} else {
    archBins.add("magisk")
    // magisk64 由 live_setup.sh 创建为 magisk 的符号链接
}
archBins.add("libzygisk_inject.so")
```

同时更新文件数校验。之前的逻辑是 `abiList.size * 8`。变更后：
- arm64-v8a: 7 文件（移除 magisk64）
- armeabi-v7a: 7 文件（移除 magisk32）
- x86_64: 7 文件（移除 magisk64）
- x86: 7 文件（移除 magisk32）

新的校验方式：`abiList.size * 7` + 还需考虑 `zygisk_inject32` 是否在检查中。

实际上 `zygisk_inject32` 不在 archBins 中——它通过 `lib/$ABI32/libzygisk_inject.so` 单独提取，不算在 APK lib 目录的 sourceFiles 中。所以总数 = `abiList.size * 7`。

```kotlin
onlyIf {
    if (inputs.sourceFiles.files.size != abiList.size * 7)
        throw StopExecutionException("Please build binaries first! (./build.py binary)")
    true
}
```

#### 文件 6：`scripts/live_setup.sh`

在部署循环前（或循环中），为 `magisk64` 和 `magisk32` 创建符号链接：

```bash
# 在提取完所有文件后、复制到 MAGISKTMP 前：
# 创建 magisk64/magisk32 符号链接（代替重复文件）
if [ ! -f magisk64 ]; then
    ln -s magisk magisk64 2>/dev/null || cp -af magisk magisk64
fi
if [ ! -f magisk32 ]; then
    ln -s magisk magisk32 2>/dev/null || cp -af magisk magisk32
fi
```

### 兼容性说明

- `init_monitor.cpp` 中 `execl(tracer, "magisk", ...)` 使用 `magisk64`/`magisk32` 作为入口——符号链接后的 `magisk64` 指向 `magisk`，行为完全一致（`exec` 调用时，内核跟随符号链接执行 `magisk`）
- `live_setup.sh` 的部署循环 `for file in magisk magisk32 magisk64 ...` 中，`magisk32` 和 `magisk64` 已是符号链接，`cp -af` 复制符号链接本身而非目标文件——所以 `/sbin/` 中的也是符号链接。`exec` 会跟随链接，行为正确。
- 如果 `ln -s` 失败（如文件系统不支持符号链接），fallback 到 `cp -af` 复制实际文件

---

## 问题三：`.init_array` 包含 NULL 条目

### 根因

NDK 的 `crtbegin_so.o` 在链接 `BUILD_SHARED_LIBRARY` 时会自动添加一个空的 `.init_array` 段。虽然所有条目都是 NULL（无构造器运行），但段的存在意味着链接器在 dlopen 时会扫描它。

### 修复

Version script（`VER { global: *; local: *; }`）不能消除 `.init_array` 段——它只控制符号可见性，与 ELF section 是否保留无关。`--gc-sections` 也无法移除被 `crtbegin` 引用的空段。

正确方案：使用链接器 script 显式丢弃 `.init_array` 段。

#### 文件 7：`native/src/Android.mk`

在 `B_ZYGISK_INJECT` 段中，在 `LOCAL_SRC_FILES` 列表之后添加：

```makefile
# 生成 linker script 以丢弃 .init_array（来自 NDK crtbegin 的空段）
$(shell echo 'SECTIONS { /DISCARD/ : { *(.init_array) *(.fini_array) } }' > $(LOCAL_PATH)/zygisk_inject/discard.ld)
LOCAL_LDFLAGS += -Wl,--script=$(LOCAL_PATH)/zygisk_inject/discard.ld
```

工作原理：链接器 script 的 `SECTIONS` 命令可以在链接阶段控制哪些 section 被保留。`/DISCARD/` 是一个伪输出 section，输入到它的 section 会被丢弃。`*(.init_array)` 匹配所有目标文件的 `.init_array` 段，包括 `crtbegin_so.o` 引入的。

这不会影响我们的 Rust 导出符号——`#[no_mangle] pub extern "C"` 函数在 `.dynsym` 表中，不受 section 丢弃影响（除非整个 section 被丢，但 `.dynsym` 和 `.text` 没有被丢弃）。

注意：`crtbegin_so.o` 中的 `.init_array` 段被丢弃后，`.fini_array`（析构数组）也一并丢弃。由于 `stubs.cpp` 中的 `__cxa_atexit` 是空操作（return 0），没有需要运行的退出处理程序，所以丢弃 `.fini_array` 也是安全的。

验证手段：
```bash
readelf -d libzygisk_inject.so | grep INIT_ARRAY
# 预期：无输出（段已被 linker script 丢弃）
```

---

## 实施顺序

三个问题互不依赖，建议按复杂度递增实施：

1. **问题三**（改 1 个文件，影响最小）→ 验证 `.init_array` 消除
2. **问题一**（改 3 个文件，功能修复）→ 需要 VPhoneOS 部署验证
3. **问题二**（改 3 个文件，构建系统变更）→ 需要 CI 验证

---

## 验证方式

| 问题 | 验证方法 |
|------|---------|
| 问题一 | 部署到 VPhoneOS，检查 magisk.log 中 32-bit `trace_zygote done` |
| 问题二 | CI 构建后检查 APK `lib/arm64-v8a/` 中无 `libmagisk64.so`；设备上 `ls -la /sbin/magisk64` 为符号链接 |
| 问题三 | CI 构建产物运行 `readelf -d libzygisk_inject.so \| grep INIT_ARRAY` 无输出 |
