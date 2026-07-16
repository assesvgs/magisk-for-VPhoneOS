#pragma once

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

/// ABI-safe 替代 lsplt::MapInfo。
///
/// lsplt::MapInfo 包含 std::string path，std::string 的 ABI（SSO vs COW）
/// 随 NDK 版本变化，导致同一类型在不同 NDK 编译的翻译单元间布局不一致。
///
/// MapEntry 用定长 char[] 存储路径，消除 ABI 差异。
/// 所有跨翻译单元边界的函数签名使用 MapEntries& 替代
/// std::vector<lsplt::MapInfo>&。
struct MapEntry {
    uintptr_t start;
    uintptr_t end;
    uint8_t perms;
    bool is_private;
    uintptr_t offset;
    dev_t dev;
    ino_t inode;
    char path[4096];  // PAGE_SIZE，覆盖 /proc/*/maps 最大路径长度
};

using MapEntries = std::vector<MapEntry>;
