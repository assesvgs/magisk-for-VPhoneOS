#pragma once

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

bool zygisk_plt_register(dev_t dev, ino_t inode, const char *symbol, void *hook, void **orig);
bool zygisk_plt_commit();
bool zygisk_plt_restore(dev_t dev, ino_t inode, const char *symbol, void *orig);

#ifdef __cplusplus
}
#endif
