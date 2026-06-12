# Kitsune Mask 集成问题记录

> 本文档记录 magisk-for-VPhoneOS 项目中发现的四个关键问题。
> 基于 30.7（我们的版本）与 27001（Kitsune Mask）的对比分析。

---

## 问题 1（P0 - 根因）：magisk 二进制未成功解压到 /sbin/

### 现象

安装 30.7 版本后重启设备，Magisk App 显示"未安装"，无 root 权限。

### 证据链

| 证据 | 30.7（不工作） | 27001（工作） |
|------|---------------|--------------|
| daemon 启动日志 | `(-1)` — 空 | 完整启动流程 |
| `/sbin/magisk*` | **不存在** | `/sbin/magisk32` + `/sbin/magisk64` 存在 |
| `/debug_ramdisk/magisk*` | **不存在** | 存在 |
| ramdisk 中的压缩文件 | `magisk.xz` (192,840 字节) ✅ | `magisk32.xz` (141,400) + `magisk64.xz` (159,212) ✅ |
| xz 格式验证 | 有效 (`fd377a585a00`) | 有效 (`fd377a585a00`) |

### 启动流程

```
init.rc 注入 → exec /sbin/magisk --post-fs-data
                    ↓
            /sbin/magisk 不存在 → exec 失败
                    ↓
            daemon 从未启动 → 日志 (-1)
                    ↓
            setup_magisk_env() 未执行
                    ↓
            /data/adb/modules 未创建 → 无 root
```

### 根因分析

`extract_files()` 函数负责解压 `magisk.xz` 到 `/sbin/magisk`：

```cpp
// native/src/init/rootdir.cpp:234-245
static void extract_files(bool sbin) {
    const char *magisk_xz = sbin ? "/sbin/magisk.xz" : "magisk.xz";
    // ...
    if (access(magisk_xz, F_OK) == 0) {
        mmap_data magisk(magisk_xz);  // ← 可能失败
        unlink(magisk_xz);
        int fd = xopen("magisk", O_WRONLY | O_CREAT, 0755);
        unxz(fd, magisk);             // ← 可能失败
        close(fd);
    }
}
```

**两个可能的失败点：**

1. **`mv_path` 未移动文件**：`mv_path(ROOTOVL "/sbin", ".")` 将 overlay.d/sbin 内容移动到当前目录。如果失败，`magisk.xz` 不在 `/sbin/` 中，`access()` 返回 -1，整个解压被跳过。

2. **`mmap_data` 静默失败**：`rust::map_file()` 在文件打开失败时返回空切片（`unwrap_or(&mut [])`），`unxz` 收到 0 字节输入直接返回 false，不创建输出文件。

### 关键差异

| 方面 | 30.7 | 27001 |
|------|------|-------|
| magiskinit 大小 | 263,856 字节 | 686,216 字节 |
| 实现语言 | Rust + C++ 混合 | 纯 C++ |
| mount 机制 | `mount_overlay` (Rust) | `magic_mount` (C++) |
| `mv_path` 调试字符串 | **无** | 有 `"mv_path "` |
| `unxz` 实现 | `write(fd, out, b.out_pos)` | `strm.write(out, b.out_pos)` |

### 修复方向

在 `extract_files()` 入口和每一步添加 `LOGD` 调试日志，确认：
- `access(magisk_xz, F_OK)` 是否返回 0
- `mmap_data` 是否成功读取文件
- `unxz` 是否成功解压
- `xopen` 是否成功创建输出文件

---

## 问题 2（P1）：direct_install 缺少关键步骤

### 现象

通过 App"直接安装"方式安装后，缺少 preinit 配置和 addon.d 生存脚本。

### 证据

**27001 的 `direct_install()`** (`manager.sh:90-110`)：
```sh
direct_install() {
  flash_image $1/new-boot.img $2
  rm -f $1/new-boot.img
  fix_env $1
  run_migrations
  copy_preinit_files    # ← 30.7 缺少
  install_addond "$3"   # ← 30.7 缺少
  return 0
}
```

**30.7 的 `direct_install()`** (`app_functions.sh:61-80`)：
```sh
direct_install() {
  flash_image $1/new-boot.img $2
  rm -f $1/new-boot.img
  fix_env $1
  run_migrations
  # 没有 copy_preinit_files！
  # 没有 install_addond！
  return 0
}
```

### 缺少步骤的作用

| 步骤 | 作用 | 影响 |
|------|------|------|
| `copy_preinit_files` | 将模块的 sepolicy.rule 复制到 preinit 分区 | 模块 SELinux 策略不生效 |
| `install_addond` | 安装 addon.d 生存脚本，复制 APK | OTA 更新后 Magisk 丢失 |

### 修复方向

在 `scripts/app_functions.sh` 的 `direct_install()` 函数末尾添加：
```sh
copy_preinit_files
install_addond "$3"
```

同时修改 `MagiskInstaller.kt` 的 `flashBoot()` 传递第三个参数（见问题 3）。

---

## 问题 3（P1）：flashBoot() 缺少 AppApkPath 参数

### 现象

直接安装时，Kotlin 层未将 APK 路径传递给 shell 脚本。

### 证据

**27001** (`MagiskInstaller.kt:511`)：
```kotlin
private fun flashBoot() = "direct_install \"$installDir\" \"$srcBoot\" \"$AppApkPath\"".sh().isSuccess
```

**30.7** (`MagiskInstaller.kt:556`)：
```kotlin
private fun flashBoot() = "direct_install $installDir $srcBoot".sh().isSuccess
```

### 差异

| 方面 | 27001 | 30.7 |
|------|-------|------|
| 参数数量 | 3 个 | 2 个 |
| `AppApkPath` | 传递 | **缺失** |
| 参数引号 | 有引号包裹 | 无引号 |
| shell 调用 | `install_addond "$3"` | 无 `$3` |

### 影响

- `install_addond` 需要 `$3`（APK 路径）来复制 APK 到 addon.d 目录
- 没有 APK 路径，addon.d 安装不完整
- OTA 后 Magisk App 可能丢失

### 修复方向

修改 `app/core/src/main/java/com/topjohnwu/magisk/core/tasks/MagiskInstaller.kt`：
```kotlin
private fun flashBoot() = "direct_install \"$installDir\" \"$srcBoot\" \"$AppApkPath\"".sh().isSuccess
```

---

## 问题 4（P2）：resolve_preinit_dir 路径映射差异

### 现象

preinit 目录解析结果不同，影响模块数据持久化位置。

### 证据

**27001** (`base/files.cpp:212-222`)：
```cpp
string resolve_preinit_dir(const char *base_dir) {
    string dir = base_dir;
    if (access((dir + "/unencrypted").data(), F_OK) == 0) {
        dir += "/unencrypted/magisk";
    } else if (access((dir + "/adb").data(), F_OK) == 0) {
        dir += "/adb/modules";    // ← 映射到 /adb/modules
    } else {
        dir += "/magisk";
    }
    return dir;
}
```

**30.7** (`native/src/base/base.cpp:396-408`)：
```cpp
string resolve_preinit_dir(const char *base_dir) {
    string dir = base_dir;
    if (access((dir + "/unencrypted").data(), F_OK) == 0) {
        dir += "/unencrypted/magisk";
    } else if (access((dir + "/adb").data(), F_OK) == 0) {
        dir += "/adb";             // ← 只映射到 /adb（缺少 /modules）
    } else if (access((dir + "/watchdog").data(), F_OK) == 0) {
        dir += "/watchdog/magisk"; // ← 新增 watchdog 分支
    } else {
        dir += "/magisk";
    }
    return dir;
}
```

### 差异

| 分区上的路径 | 27001 解析结果 | 30.7 解析结果 |
|-------------|---------------|---------------|
| `/adb` | `<partition>/adb/modules` | `<partition>/adb` |
| `/unencrypted` | `<partition>/unencrypted/magisk` | `<partition>/unencrypted/magisk`（相同） |
| `/watchdog` | 无此分支 | `<partition>/watchdog/magisk`（新增） |

### 影响

当 preinit 分区上有 `/adb` 目录时：
- 27001 将 preinit 数据放在 `<partition>/adb/modules/`
- 30.7 将 preinit 数据放在 `<partition>/adb/`
- 这可能导致模块 sepolicy 规则和 early-mount 数据的存储位置不同

### 修复方向

将 30.7 的 `resolve_preinit_dir` 修改为与 27001 一致：
```cpp
} else if (access((dir + "/adb").data(), F_OK) == 0) {
    dir += "/adb/modules";    // 改为 /adb/modules
}
```

---

## 优先级总结

| 优先级 | 问题 | 影响范围 | 修复难度 |
|--------|------|----------|----------|
| 🔴 P0 | magisk 二进制未解压 | 根因 — 无 root | 需调试日志定位 |
| 🟡 P1 | direct_install 缺少步骤 | 安装配置 | 低 — 添加两行代码 |
| 🟡 P1 | flashBoot 缺少参数 | 安装配置 | 低 — 修改一行代码 |
| 🟡 P2 | resolve_preinit_dir 差异 | 模块持久化 | 低 — 修改路径字符串 |

---

## 测试环境

- 设备：VPhoneOS VM (HONOR PGT-AN10, Android 10, API 29)
- 内核：Linux aarch64 4.14.42-super
- Magisk 版本：30700 (30.7)
- Kitsune Mask 版本：27001
- 安装方式：App 直接安装

## 相关文件

- 安装日志：`日志/30.7/debug/magisk_install_log_2026-06-12T15.41.17.log`
- daemon 日志：`日志/30.7/debug/magisk_log_2026-06-12T15.42.12.log`
- 27001 daemon 日志：`日志/27001/magisk_log_2026-06-12T15.59.36.log`
- 30.7 boot 镜像：`boot/30.1/mtdblock.img`, `boot/30.1/ramdisk.img`
- 27001 boot 镜像：`boot/27001/mtdblock.img`, `boot/27001/ramdisk.img`
- 30.7 二进制：`30.1/magisk`, `30.1/magiskinit`, `30.1/init-ld`
- 27001 二进制：`27001/magisk64`, `27001/magiskinit`
