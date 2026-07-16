# 修改文件清单

## 新增文件（原版没有）

| 文件 | 说明 |
|------|------|
| `core/kitsune.cpp` | Kitsune Mask 全局变量（magisktmpfs_fd、su_bin_fd、HAVE_32、logging_muted）和函数（enable_mount_su、su_mount、tmpfs_mount、bind_mount_）。mount_mirrors 已删除——未实现，未调用，由 Rust module 挂载逻辑替代。 |
| `core/deny/ptrace.cpp` | ptrace 进程监控实现，替代原版的 logcat.cpp |
| `core/deny/revert.cpp` | /sbin 挂载/卸载、SoList 隐藏、revert_daemon（revert_unmount 已移至 Rust） |
| `zygisk/solist.hpp` | SoList 隐藏功能，从 linker 链表中抹掉 Zygisk 模块路径 |
| `zygisk/memory.cpp` | JNI hook 内存分配器实现（mmap 单调分配器） |
| `zygisk/memory.hpp` | JNI hook 内存分配器头文件 |

## 删除文件（原版有）

| 文件 | 原因 |
|------|------|
| `core/deny/logcat.cpp` | 被 ptrace.cpp 替代 |
| `PROGRESS.md` | 归档至 docs/，设计文档已完成使命 |
| `e5afe2af-symbols/` | 构建产物，不应版本控制 |

## 修改文件

| 文件 | 修改内容 |
|------|---------|
| `native/src/Android.mk` | 构建列表：logcat.cpp → ptrace.cpp + revert.cpp + kitsune.cpp + memory.cpp；添加 libphmap、libxdl 依赖 |
| `native/src/external/Android.mk` | 添加 libphmap 和 libxdl 构建规则 |
| `native/src/Application.mk` | 添加 APP_ABI（四架构）、APP_SUPPORT_FLEXIBLE_PAGE_SIZES（16KB 页面）、B_BB（Busybox） |
| `core/include/core.hpp` | 添加 Kitsune Mask 声明（revert_unmount、magisktmpfs_fd、su_bin_fd、logging_muted、su_mount、enable_mount_su、tmpfs_mount、bind_mount_）；mount_mirrors 声明已删除（未实现，已由 Rust 替代）；mount_info 和 parse_mount_info 已移至 Rust |
| `core/deny/deny.hpp` | 添加 SIGTERMTHRD 宏、sulist 枚举、crawl_procfs/is_uid_on_list/rescan_apps/revert_daemon/mount_magisk_to_pid/do_mount_magisk 声明 |
| `core/deny/cli.cpp` | 命令从 --denylist 改为 --hide，添加 sulist、--do-unmount、--mount-sbin、--setup-sbin |
| `core/deny/utils.cpp` | 添加 sulist 功能、rescan_apps、is_uid_on_list、is_deny_target 三参数版本、proc_monitor 线程启动 |
| `core/lib.rs` | 添加 ZygiskRequest（SulistRootNs、RevertUnmount）、ZygiskStateFlags（ProcessOnAllowList、AllowlistEnforcing、DoRevertUnmount、DoAllow）、enable_mount_su FFI 声明；revert_unmount 改为 Rust 导出 |
| `core/magisk.rs` | --denylist 改为 --hide |
| `core/mount.rs` | 添加 Rust 版本 revert_unmount（支持 4 种卸载类型）、find_preinit_device_sysfs、诊断日志 |
| `core/bootstages.rs` | 添加 enable_mount_su() 导入和调用 |
| `zygisk/entry.cpp` | 添加 remote_request_sulist() 和 remote_request_umount() |
| `zygisk/hook.cpp` | 添加 sulist 逻辑、SoList 隐藏调用、内存释放调用 |
| `zygisk/module.cpp` | 添加 sulist 模式判断、SoList 隐藏、内存重映射隐藏 |
| `zygisk/module.hpp` | 添加 DO_ALLOW 标志 |
| `zygisk/zygisk.hpp` | 添加 sulist 函数声明、zygisk_request 声明 |
| `include/consts.hpp` | 包名改为 io.github.huskydg.magisk，添加 MODULEMNT、EARLYMNT、EARLYMNTNAME |
| `init/bootstages.rs` | 添加 enable_mount_su 导入和调用 |
| `init/rootdir.rs` | 添加 start logd 和 zygote-restart 特性 |
| `app/.../SettingsItems.kt` | 添加 MagiskHide 和 SuList 设置项 |
| `app/.../SettingsViewModel.kt` | 注册 MagiskHide 和 SuList |
| `app/.../Config.kt` | 添加 sulist 配置属性 |
| `app/.../Info.kt` | 添加 sulist 状态检查 |
| `app/.../DenyListFragment.kt` | 根据 sulist 状态切换标题 |
| `app/.../strings.xml` | 添加 MagiskHide、SuList 相关字符串 |
| `app/shared/AndroidManifest.xml` | 应用名称改为 Kitsune Mask |
| `app/core/res/drawable/ic_magisk.xml` | 图标改为 Kitsune Mask 狐狸 |
| `base/files.rs` | 新增 `is_vphoneos()` 统一 VPhoneOS 检测 |
| `core/daemon.rs` | VPhoneOS 检测改用 `base::is_vphoneos()` |
| `core/su/daemon.rs` | VPhoneOS 检测改用 `base::is_vphoneos()` |
| `core/kitsune.cpp` | `mount_mirrors()` 已删除（无调用点，由 Rust 模块挂载替代） |
| `core/include/core.hpp` | `mount_mirrors` 声明已删除 |
| `core/deny/revert.cpp` | `mount_mirrors` 前向声明已删除 |
| `core/zygisk/init_monitor.cpp` | `exec_tracer` 超时杀子进程 + `inject_zygote` SIGCONT 恢复 + polling 成功判断 |
| `README.MD` | 更新历史根因记录，添加 VPhoneOS 兼容性章节 |
