#include "plt_hook.h"
#include <lsplt.hpp>

bool zygisk_plt_register(dev_t dev, ino_t inode, const char *symbol, void *hook, void **orig) {
    return lsplt::RegisterHook(dev, inode, symbol, hook, orig);
}

bool zygisk_plt_commit() {
    return lsplt::CommitHook();
}
