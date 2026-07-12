#!/usr/bin/env python3
"""Generate proxy_gen.rs from method signatures.

Usage: python3 scripts/gen_jni_hooks.py > native/src/zygisk_inject/src/proxy_gen.rs
"""

ind = lambda n: '\n' + '    ' * n

def camel_to_snake(name: str) -> str:
    """Convert camelCase to snake_case.
    e.g. 'nativeForkAndSpecialize' -> 'native_fork_and_specialize'
    """
    result = []
    for i, c in enumerate(name):
        if c.isupper() and i > 0:
            result.append('_')
            result.append(c.lower())
        else:
            result.append(c.lower())
    return ''.join(result)


class JType:
    def __init__(self, cpp, jni, rust):
        self.cpp = cpp   # C++ type name (for reference)
        self.jni = jni   # JNI signature character
        self.rust = rust # Rust type path


class JArray(JType):
    def __init__(self, elem):
        super().__init__(
            'jobjectArray',
            '[' + elem.jni,
            '*mut c_void',
        )


class Argument:
    def __init__(self, name, jtype):
        self.name = name
        self.type = jtype

    def rust_param(self):
        return f'{self.name}: {self.type.rust}'


class Anon(Argument):
    cnt = 0
    def __init__(self, jtype):
        name = f'_{Anon.cnt}'
        Anon.cnt += 1
        super().__init__(name, jtype)


# JNI primitive types
jint = JType('jint', 'I', 'jint')
jboolean = JType('jboolean', 'Z', 'jboolean')
jlong = JType('jlong', 'J', 'jlong')
jstring = JType('jstring', 'Ljava/lang/String;', '*mut c_void')
jintArray = JType('jintArray', '[I', '*mut c_void')
jlongArray = JType('jlongArray', '[J', '*mut c_void')
jobjectArray = JType('jobjectArray', '[Ljava/lang/Object;', '*mut c_void')
void = JType('void', 'V', '()')

# Common args
uid = Argument('uid', jint)
gid = Argument('gid', jint)
gids = Argument('gids', jintArray)
runtime_flags = Argument('runtime_flags', jint)
rlimits = Argument('rlimits', JArray(jintArray))  # jobjectArray => [[I
mount_external = Argument('mount_external', jint)
se_info = Argument('se_info', jstring)
nice_name = Argument('nice_name', jstring)
fds_to_close = Argument('fds_to_close', jintArray)
fds_to_ignore = Argument('fds_to_ignore', jintArray)
instruction_set = Argument('instruction_set', jstring)
app_data_dir = Argument('app_data_dir', jstring)
is_child_zygote = Argument('is_child_zygote', jboolean)
is_top_app = Argument('is_top_app', jboolean)
pkg_data_info_list = Argument('pkg_data_info_list', JArray(jstring))
whitelisted_data_info_list = Argument('whitelisted_data_info_list', JArray(jstring))
mount_data_dirs = Argument('mount_data_dirs', jboolean)
mount_storage_dirs = Argument('mount_storage_dirs', jboolean)
mount_sysprop_overrides = Argument('mount_sysprop_overrides', jboolean)
permitted_capabilities = Argument('permitted_capabilities', jlong)
effective_capabilities = Argument('effective_capabilities', jlong)


class Method:
    def __init__(self, base_name, suffix, ret, args):
        self.base_name = base_name
        self.suffix = suffix
        self.func_name = f'{base_name}_{suffix}'
        self.ret = ret
        self.args = args

    def jni_sig(self):
        a = ''.join(a.type.jni for a in self.args)
        return f'({a}){self.ret.jni}'

    def rust_params(self):
        # 不加 mut——用于函数指针类型声明（fn pointer type 不允许 mut）
        params = [f'env: *mut c_void', f'clazz: jclass']
        for a in self.args:
            params.append(a.rust_param())
        return ', '.join(params)

    def rust_mut_params(self):
        # 加 mut——用于函数定义，因为 proxy 函数体用 addr_of_mut! 取每个参数地址
        # 但匿名参数（_0, _1 等 Samsung 额外参数）不加 mut——它们不会传给 addr_of_mut!
        params = [f'mut env: *mut c_void', f'mut clazz: jclass']
        for a in self.args:
            if a.name.startswith('_'):
                params.append(f'{a.name}: {a.type.rust}')
            else:
                params.append(f'mut {a.name}: {a.type.rust}')
        return ', '.join(params)

    def rust_call_args(self):
        return ', '.join(['env', 'clazz'] + [a.name for a in self.args])

    def rust_init_args(self, prefix='args'):
        """Generate AppSpecializeArgs field initialization."""
        fields = []
        # These are the base fields from the constructor
        base_field_names = [
            'uid', 'gid', 'gids', 'runtime_flags', 'rlimits',
            'mount_external', 'se_info', 'nice_name',
            'instruction_set', 'app_data_dir',
        ]
        for f in base_field_names:
            fields.append(f'{f}: core::ptr::addr_of_mut!({f}) as *mut c_void')

        for a in self.args:
            name = a.name
            # Skip base fields already initialized
            if name in base_field_names:
                continue
            # Also skip _N anonymous args (Samsung extras)
            if name.startswith('_'):
                continue
            fields.append(f'{name}: core::ptr::addr_of_mut!({name}) as *mut c_void')

        return ind(1).join(f'{prefix}.{f};\n' for f in fields)


# ====== Method definitions ======

# nativeForkAndSpecialize variants (11)
fas_l = Method('nativeForkAndSpecialize', 'l', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, instruction_set, app_data_dir])
fas_o = Method('nativeForkAndSpecialize', 'o', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, instruction_set, app_data_dir])
fas_p = Method('nativeForkAndSpecialize', 'p', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, is_child_zygote,
    instruction_set, app_data_dir])
fas_q_alt = Method('nativeForkAndSpecialize', 'q_alt', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, is_child_zygote,
    instruction_set, app_data_dir, is_top_app])
fas_r = Method('nativeForkAndSpecialize', 'r', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, is_child_zygote,
    instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs])
fas_u = Method('nativeForkAndSpecialize', 'u', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, is_child_zygote,
    instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs,
    mount_sysprop_overrides])
# Samsung
fas_samsung_m = Method('nativeForkAndSpecialize', 'samsung_m', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, Anon(jint), Anon(jint), nice_name, fds_to_close,
    instruction_set, app_data_dir])
fas_samsung_n = Method('nativeForkAndSpecialize', 'samsung_n', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, Anon(jint), Anon(jint), nice_name, fds_to_close,
    instruction_set, app_data_dir, Anon(jint)])
fas_samsung_o = Method('nativeForkAndSpecialize', 'samsung_o', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, Anon(jint), Anon(jint), nice_name, fds_to_close,
    fds_to_ignore, instruction_set, app_data_dir])
fas_samsung_p = Method('nativeForkAndSpecialize', 'samsung_p', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, Anon(jint), Anon(jint), nice_name, fds_to_close,
    fds_to_ignore, is_child_zygote, instruction_set, app_data_dir])
# GrapheneOS
fas_grapheneos_u = Method('nativeForkAndSpecialize', 'grapheneos_u', jint, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, fds_to_close, fds_to_ignore, is_child_zygote,
    instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs,
    mount_sysprop_overrides, Anon(jlongArray)])

# nativeSpecializeAppProcess variants (6)
spec_q = Method('nativeSpecializeAppProcess', 'q', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, is_child_zygote, instruction_set, app_data_dir])
spec_q_alt = Method('nativeSpecializeAppProcess', 'q_alt', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, is_child_zygote, instruction_set, app_data_dir, is_top_app])
spec_r = Method('nativeSpecializeAppProcess', 'r', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, is_child_zygote, instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs])
spec_u = Method('nativeSpecializeAppProcess', 'u', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, is_child_zygote, instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs,
    mount_sysprop_overrides])
spec_samsung_q = Method('nativeSpecializeAppProcess', 'samsung_q', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, Anon(jint), Anon(jint), nice_name, is_child_zygote,
    instruction_set, app_data_dir])
spec_grapheneos_u = Method('nativeSpecializeAppProcess', 'grapheneos_u', void, [
    uid, gid, gids, runtime_flags, rlimits, mount_external,
    se_info, nice_name, is_child_zygote, instruction_set, app_data_dir, is_top_app,
    pkg_data_info_list, whitelisted_data_info_list, mount_data_dirs, mount_storage_dirs,
    mount_sysprop_overrides, Anon(jlongArray)])

# nativeForkSystemServer variants (2)
server_l = Method('nativeForkSystemServer', 'l', jint, [
    uid, gid, gids, runtime_flags, rlimits,
    permitted_capabilities, effective_capabilities])
server_samsung_q = Method('nativeForkSystemServer', 'samsung_q', jint, [
    uid, gid, gids, runtime_flags, Anon(jint), Anon(jint), rlimits,
    permitted_capabilities, effective_capabilities])

ALL_METHODS = [
    fas_l, fas_o, fas_p, fas_q_alt, fas_r, fas_u,
    fas_samsung_m, fas_samsung_n, fas_samsung_o, fas_samsung_p, fas_grapheneos_u,
    spec_q, spec_q_alt, spec_r, spec_u, spec_samsung_q, spec_grapheneos_u,
    server_l, server_samsung_q,
]


def gen_app_specialize_args():
    """Generate AppSpecializeArgs struct."""
    fields = [
        'uid', 'gid', 'gids', 'runtime_flags', 'rlimits',
        'mount_external', 'se_info', 'se_name', 'nice_name',
        'managed_nice_name', 'instruction_set', 'app_data_dir',
        'fds_to_close', 'fds_to_ignore',
        'is_child_zygote', 'is_top_app',
        'pkg_data_info_list', 'whitelisted_data_info_list',
        'mount_data_dirs', 'mount_storage_dirs',
        'mount_sysprop_overrides',
    ]
    out = '#[repr(C)]\n'
    out += 'pub struct AppSpecializeArgs {\n'
    for f in fields:
        out += f'    pub {f}: *mut c_void,\n'
    out += '}\n'
    return out


def gen_server_specialize_args():
    """Generate ServerSpecializeArgs struct."""
    out = '#[repr(C)]\n'
    out += 'pub struct ServerSpecializeArgs {\n'
    out += '    pub uid: *mut c_void,\n'
    out += '    pub gid: *mut c_void,\n'
    out += '    pub gids: *mut c_void,\n'
    out += '    pub runtime_flags: *mut c_void,\n'
    out += '    pub rlimits: *mut c_void,\n'
    out += '    pub permitted_capabilities: *mut c_void,\n'
    out += '    pub effective_capabilities: *mut c_void,\n'
    out += '}\n'
    return out


def gen_jni_method_entry():
    """Generate JniMethodEntry struct."""
    out = 'pub struct JniMethodEntry {\n'
    out += '    pub name: &\'static str,\n'
    out += '    pub sig: &\'static str,\n'
    out += '    pub orig_idx: usize,\n'
    out += '    pub handler: *mut c_void,\n'
    out += '}\n'
    out += '// JniMethodEntry 只读，不修改，Sync 安全\n'
    out += 'unsafe impl Sync for JniMethodEntry {}\n'
    return out


def gen_table():
    """Generate ORIG_PTRS array and JNI_METHOD_TABLE."""
    out = f'pub const TABLE_SIZE: usize = {len(ALL_METHODS)};\n\n'
    out += 'struct OrigPtrs(UnsafeCell<[*mut c_void; TABLE_SIZE]>);\n'
    out += 'unsafe impl Sync for OrigPtrs {}\n\n'
    out += 'pub static ORIG_PTRS: OrigPtrs =\n'
    out += '    OrigPtrs(UnsafeCell::new([core::ptr::null_mut(); TABLE_SIZE]));\n\n'
    out += 'pub fn set_orig_ptr(i: usize, ptr: *mut c_void) {\n'
    out += '    unsafe { (*ORIG_PTRS.0.get())[i] = ptr; }\n'
    out += '}\n\n'
    out += 'pub fn get_orig_ptr(i: usize) -> *mut c_void {\n'
    out += '    unsafe { (*ORIG_PTRS.0.get())[i] }\n'
    out += '}\n\n'
    out += 'pub static JNI_METHOD_TABLE: [JniMethodEntry; TABLE_SIZE] = [\n'
    for idx, m in enumerate(ALL_METHODS):
        out += f'    JniMethodEntry {{\n'
        out += f'        name: "{m.base_name}",\n'
        out += f'        sig: "{m.jni_sig()}",\n'
        out += f'        orig_idx: {idx},\n'
        out += f'        handler: {m.func_name} as *mut c_void,\n'
        out += f'    }},\n'
    out += '];\n'
    return out


def gen_hook_and_save():
    """Generate hook_and_save_zygote_methods."""
    out = 'pub unsafe fn hook_and_save_zygote_methods(\n'
    out += '    env: *mut c_void,\n'
    out += '    methods: *mut crate::jni_env::JNINativeMethod,\n'
    out += '    n_methods: jint,\n'
    out += ') {\n'
    out += '    if methods.is_null() || n_methods <= 0 { return; }\n'
    out += '    for i in 0..n_methods as isize {\n'
    out += '        let m = &mut *methods.offset(i);\n'
    out += '        let m_name = core::ffi::CStr::from_ptr(m.name).to_str().unwrap_or("");\n'
    out += '        let m_sig = core::ffi::CStr::from_ptr(m.signature).to_str().unwrap_or("");\n'
    out += '        for entry in &JNI_METHOD_TABLE {\n'
    out += '            if entry.name == m_name && entry.sig == m_sig {\n'
    out += '                set_orig_ptr(entry.orig_idx, m.fn_ptr);\n'
    out += '                m.fn_ptr = entry.handler;\n'
    out += '                break;\n'
    out += '            }\n'
    out += '        }\n'
    out += '    }\n'
    out += '}\n'
    return out


def is_fork(m):
    """Check if method is nativeForkAndSpecialize variant."""
    return m.base_name == 'nativeForkAndSpecialize'


def is_server(m):
    """Check if method is nativeForkSystemServer variant."""
    return m.base_name == 'nativeForkSystemServer'


def is_specialize(m):
    """Check if method is nativeSpecializeAppProcess variant."""
    return m.base_name == 'nativeSpecializeAppProcess'


def gen_proxy_function_inline(m):
    """Generate a proxy function with inline pre/post/call (no helper)."""
    out = f'#[no_mangle]\n'
    out += f'pub unsafe extern "C" fn {m.func_name}(\n'
    out += f'    {m.rust_mut_params()}\n'
    out += f') -> {m.ret.rust} {{\n'

    idx = ALL_METHODS.index(m)
    out += f'    let orig = get_orig_ptr({idx});\n'
    if m.ret.rust != '()':
        out += '    if orig.is_null() { return -1; }\n'
    else:
        out += '    if orig.is_null() { return; }\n'

    out += f'    let orig_fn: unsafe extern "C" fn({m.rust_params()}) -> {m.ret.rust}\n'
    out += f'        = core::mem::transmute(orig);\n'

    if is_specialize(m) or is_fork(m):
        out += f'    let mut args = AppSpecializeArgs {{\n'
        base_fields = [
            'uid', 'gid', 'gids', 'runtime_flags', 'rlimits',
            'mount_external', 'se_info', 'se_name', 'nice_name',
            'managed_nice_name', 'instruction_set', 'app_data_dir',
            'fds_to_close', 'fds_to_ignore',
            'is_child_zygote', 'is_top_app',
            'pkg_data_info_list', 'whitelisted_data_info_list',
            'mount_data_dirs', 'mount_storage_dirs',
            'mount_sysprop_overrides',
        ]
        for f in base_fields:
            matched = None
            for a in m.args:
                if a.name == f:
                    matched = a
                    break
            if matched:
                out += f'        {f}: core::ptr::addr_of_mut!({matched.name}) as *mut c_void,\n'
            else:
                out += f'        {f}: core::ptr::null_mut(),\n'
        out += '    };\n'
        out += f'    let mut ctx = HookContext::new(env, core::ptr::addr_of_mut!(args) as *mut c_void, "com.android.internal.os.Zygote");\n'
        out += '    let _prev_ctx = crate::hook_context::get_current_ptr();\n'
        out += '    crate::hook_context::set_current(&mut ctx);\n'
        pre_name = camel_to_snake(m.base_name)
        out += f'    ctx.{pre_name}_pre();\n'
        out += f'    let _ret = orig_fn({m.rust_call_args()});\n'

        if is_fork(m):
            out += '    if _ret == 0 {\n'
            out += '        ctx.native_specialize_app_process_post();\n'
            out += '        crate::unload::SHOULD_UNLOAD.store(true, Ordering::Release);\n'
            out += '    } else {\n'
            out += '        ctx.app_specialize_post();\n'
            out += '    }\n'
            out += '    crate::hook_context::set_current_ptr(_prev_ctx);\n'
            out += '    _ret\n'
        elif is_specialize(m):
            out += '    ctx.native_specialize_app_process_post();\n'
            out += '    crate::hook_context::set_current_ptr(_prev_ctx);\n'
        else:  # server
            out += '    ctx.native_fork_system_server_post();\n'
            out += '    crate::hook_context::set_current_ptr(_prev_ctx);\n'
            out += '    _ret\n'
    else:  # nativeForkSystemServer
        out += f'    let mut args = ServerSpecializeArgs {{\n'
        server_fields = ['uid', 'gid', 'gids', 'runtime_flags', 'rlimits',
                         'permitted_capabilities', 'effective_capabilities']
        for f in server_fields:
            matched = None
            for a in m.args:
                if a.name == f:
                    matched = a
                    break
            if matched:
                out += f'        {f}: core::ptr::addr_of_mut!({matched.name}) as *mut c_void,\n'
            else:
                out += f'        {f}: core::ptr::null_mut(),\n'
        out += '    };\n'
        out += f'    let mut ctx = HookContext::new(env, core::ptr::addr_of_mut!(args) as *mut c_void, "com.android.internal.os.Zygote");\n'
        out += '    let _prev_ctx = crate::hook_context::get_current_ptr();\n'
        out += '    crate::hook_context::set_current(&mut ctx);\n'
        out += f'    ctx.native_fork_system_server_pre();\n'
        out += f'    let _ret = orig_fn({m.rust_call_args()});\n'
        out += '    if _ret == 0 {\n'
        out += '        ctx.server_specialize_post();\n'
        out += '        crate::unload::SHOULD_UNLOAD.store(true, Ordering::Release);\n'
        out += '    } else {\n'
        out += '        ctx.fork_post();\n'
        out += '    }\n'
        out += '    crate::hook_context::set_current_ptr(_prev_ctx);\n'
        out += '    _ret\n'

    out += '}\n'
    return out


def gen_all():
    out = '// Generated by scripts/gen_jni_hooks.py\n'
    out += '// Do not edit manually.\n\n'
    out += 'use core::ffi::c_void;\n'
    out += 'use core::sync::atomic::Ordering;\n'
    out += 'use core::cell::UnsafeCell;\n'
    out += 'use core::ptr;\n'
    out += 'use core::mem;\n'
    out += 'use crate::jni_env::{jclass, jint, jlong, jboolean};\n'
    out += 'use crate::hook_context::{HookContext, get_current_ptr, set_current, set_current_ptr};\n\n'

    out += gen_app_specialize_args()
    out += '\n'
    out += gen_server_specialize_args()
    out += '\n'
    out += gen_jni_method_entry()
    out += '\n'
    out += gen_table()
    out += '\n'
    out += gen_hook_and_save()
    out += '\n'

    for m in ALL_METHODS:
        out += gen_proxy_function_inline(m)
        out += '\n'

    out = out.rstrip() + '\n'
    return out


if __name__ == '__main__':
    print(gen_all())
