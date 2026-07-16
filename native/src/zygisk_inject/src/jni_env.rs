use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicBool, Ordering};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

pub type JavaVM = *mut c_void;
pub type jclass = *mut c_void;
pub type jint = i32;
pub type jlong = i64;
pub type jboolean = u8;
pub type jmethodID = *mut c_void;
pub type jstring = *mut c_void;

#[repr(C)]
pub struct JNINativeMethod {
    pub name: *const libc::c_char,
    pub signature: *const libc::c_char,
    pub fn_ptr: *mut c_void,
}

static GLOBAL_JAVA_VM: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static ORIG_FUNCTIONS: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static ORIG_REGISTER_NATIVES: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static JNI_HOOKED: AtomicBool = AtomicBool::new(false);

pub struct JniOffsets {
    pub get_object_class: usize,
    pub get_method_id: usize,
    pub call_object_method: usize,
    pub get_string_utf_chars: usize,
    pub release_string_utf_chars: usize,
}

struct JniOffsetsCell {
    inited: AtomicBool,
    data: UnsafeCell<MaybeUninit<JniOffsets>>,
}
// JniOffsetsCell 的 init() 在 hook_jni_env 中单线程调用，
// 之后 get() 由 Release/Acquire 同步保护。UnsafeCell 本身不 Sync，
// 但此处通过 AtomicBool + 严格 Release/Acquire 确保安全。
unsafe impl Sync for JniOffsetsCell {}

impl JniOffsetsCell {
    const fn new() -> Self {
        Self {
            inited: AtomicBool::new(false),
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn init(&self, offsets: JniOffsets) {
        unsafe { (*self.data.get()).write(offsets); }
        self.inited.store(true, Ordering::Release);
    }

    fn get(&self) -> Option<&JniOffsets> {
        if self.inited.load(Ordering::Acquire) {
            Some(unsafe { (*self.data.get()).assume_init_ref() })
        } else {
            None
        }
    }
}

static JNI_OFFSETS: JniOffsetsCell = JniOffsetsCell::new();

pub fn get_orig_register_natives() -> Option<
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut JNINativeMethod, jint) -> jint
> {
    let ptr = ORIG_REGISTER_NATIVES.load(Ordering::Relaxed);
    if ptr.is_null() { None } else { Some(unsafe { core::mem::transmute(ptr) }) }
}

fn find_jni_function_offset(functions: *mut c_void, sym_name: &[u8]) -> Option<usize> {
    let addr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, sym_name.as_ptr() as *const libc::c_char) };
    if addr.is_null() {
        // 符号未导出——可能在此 Android 版本上不可见
        // 调用方将 fallback 到硬编码偏移
        return None;
    }
    let max_entries = 256;
    for i in 0..max_entries {
        if unsafe { *(functions as *mut *mut c_void).add(i) } == addr {
            return Some(i);
        }
    }
    None
}

pub fn init_jni_offsets(functions: *mut c_void) -> bool {
    let get_obj = find_jni_function_offset(functions, b"GetObjectClass\0");
    let get_mid = find_jni_function_offset(functions, b"GetMethodID\0");
    let call_obj = find_jni_function_offset(functions, b"CallObjectMethod\0");
    let get_str = find_jni_function_offset(functions, b"GetStringUTFChars\0");
    let release_str = find_jni_function_offset(functions, b"ReleaseStringUTFChars\0");

    // fallback 到标准 JNINativeInterface 偏移
    // 注意：这些偏移在 AOSP JNI 规范中标准化，但 OEM 可能修改 ART 实现。
    // 若 dlsym 找不到任何符号，get_class_name 将使用硬编码值——在非标准 ART 上可能失败。
    let offsets = JniOffsets {
        get_object_class: get_obj.unwrap_or(31),
        get_method_id: get_mid.unwrap_or(33),
        call_object_method: call_obj.unwrap_or(34),
        get_string_utf_chars: get_str.unwrap_or(69),
        release_string_utf_chars: release_str.unwrap_or(70),
    };
    JNI_OFFSETS.init(offsets);
    true
}

pub fn get_global_jni_env() -> *mut c_void {
    let vm = GLOBAL_JAVA_VM.load(Ordering::Acquire);
    if vm.is_null() {
        return core::ptr::null_mut();
    }
    type GetEnvFn = unsafe extern "C" fn(*mut c_void, *mut *mut c_void, i32) -> i32;
    let vtable = unsafe { *(vm as *mut *mut c_void) };
    let get_env: GetEnvFn = unsafe { core::mem::transmute(*((vtable as *mut *mut c_void).add(6))) };
    let mut env: *mut c_void = core::ptr::null_mut();
    if unsafe { get_env(vm, &mut env, 0x00010006) } != 0 {
        return core::ptr::null_mut();
    }
    env
}

fn get_class_name(env: *mut c_void, clazz: jclass) -> Option<alloc::string::String> {
    let offsets = JNI_OFFSETS.get()?;
    let functions = unsafe { *(env as *mut *mut c_void) } as *mut *mut c_void;
    if functions.is_null() { return None; }

    type GetObjectClassFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type GetMethodIDFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const i8, *const i8) -> *mut c_void;
    type CallObjectMethodFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type GetStringUTFCharsFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *const i8;
    type ReleaseStringUTFCharsFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const i8);

    let get_obj_class: GetObjectClassFn = unsafe { core::mem::transmute(functions.add(offsets.get_object_class)) };
    let get_mid: GetMethodIDFn = unsafe { core::mem::transmute(functions.add(offsets.get_method_id)) };
    let call_obj: CallObjectMethodFn = unsafe { core::mem::transmute(functions.add(offsets.call_object_method)) };
    let get_str: GetStringUTFCharsFn = unsafe { core::mem::transmute(functions.add(offsets.get_string_utf_chars)) };
    let release_str: ReleaseStringUTFCharsFn = unsafe { core::mem::transmute(functions.add(offsets.release_string_utf_chars)) };

    let class_class = unsafe { get_obj_class(env, clazz) };
    if class_class.is_null() { return None; }

    let name = b"getName\0".as_ptr() as *const i8;
    let sig = b"()Ljava/lang/String;\0".as_ptr() as *const i8;
    let mid = unsafe { get_mid(env, class_class, name, sig) };
    if mid.is_null() { return None; }

    let str_obj = unsafe { call_obj(env, clazz, mid) };
    if str_obj.is_null() { return None; }

    let utf_chars = unsafe { get_str(env, str_obj, core::ptr::null_mut()) };
    if utf_chars.is_null() { return None; }

    let result = unsafe { core::ffi::CStr::from_ptr(utf_chars as *const core::ffi::c_char) }.to_str().ok().map(|s| alloc::string::String::from(s));
    unsafe { release_str(env, str_obj, utf_chars) };
    result
}

pub unsafe extern "C" fn hook_register_natives(
    env: *mut c_void,
    clazz: jclass,
    methods: *mut JNINativeMethod,
    n_methods: jint,
) -> jint {
    let orig_ptr = ORIG_REGISTER_NATIVES.load(Ordering::Relaxed);
    if orig_ptr.is_null() {
        return 0;
    }
    let orig_fn: unsafe extern "C" fn(*mut c_void, jclass, *mut JNINativeMethod, jint) -> jint =
        core::mem::transmute(orig_ptr);

    let class_name = get_class_name(env, clazz);
    if class_name.as_deref() == Some("com.android.internal.os.Zygote") {
        crate::proxy_gen::hook_and_save_zygote_methods(env, methods, n_methods);
    }

    orig_fn(env, clazz, methods, n_methods)
}

pub fn hook_jni_env() -> bool {
    if JNI_HOOKED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return true;
    }

    type JniGetVms = unsafe extern "C" fn(*mut *mut c_void, i32, *mut i32) -> i32;
    let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"JNI_GetCreatedJavaVMs\0".as_ptr() as *const libc::c_char) };
    if sym.is_null() {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }
    let get_vms: JniGetVms = unsafe { core::mem::transmute(sym) };

    let mut vm: *mut c_void = core::ptr::null_mut();
    let mut count: i32 = 0;
    if unsafe { get_vms(&mut vm, 1, &mut count) } != 0 || count == 0 || vm.is_null() {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }

    type GetEnvFn = unsafe extern "C" fn(*mut c_void, *mut *mut c_void, i32) -> i32;
    let vtable = unsafe { *(vm as *mut *mut c_void) };
    let get_env: GetEnvFn = unsafe { core::mem::transmute(*((vtable as *mut *mut c_void).add(6))) };

    let mut env: *mut c_void = core::ptr::null_mut();
    if unsafe { get_env(vm, &mut env, 0x00010006) } != 0 || env.is_null() {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }

    let old_functions = unsafe { *(env as *const *mut c_void) };
    if old_functions.is_null() {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }

    // JNINativeInterface 前 4 个条目是 reserved0-3（都是 NULL），不能用 NULL 扫描确定大小。
    // 标准 JNI 规范定义了 ~250 个函数指针（Android 10 API 29 有 251 个条目，索引 0-250）。
    // RegisterNatives 在索引 ~215（64-bit），需要足够大的固定大小。
    const JNI_TABLE_ENTRIES: usize = 256;
    let table_size = JNI_TABLE_ENTRIES * core::mem::size_of::<*mut c_void>();
    let new_functions = unsafe {
        libc::mmap(
            core::ptr::null_mut(),
            table_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if new_functions == libc::MAP_FAILED {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(old_functions as *const u8, new_functions as *mut u8, table_size);
    }

    GLOBAL_JAVA_VM.store(vm, Ordering::Release);
    ORIG_FUNCTIONS.store(old_functions, Ordering::Release);

    init_jni_offsets(new_functions);

    let reg_sym = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, b"JNI_RegisterNatives\0".as_ptr() as *const libc::c_char)
    };
    if reg_sym.is_null() {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }

    let mut found = false;
    for i in 0..256 {
        let entry = unsafe { *(new_functions as *mut *mut c_void).add(i) };
        if entry == reg_sym {
            ORIG_REGISTER_NATIVES.store(entry, Ordering::Release);
            unsafe {
                *(new_functions as *mut *mut c_void).add(i) = hook_register_natives as *mut c_void;
            }
            found = true;
            break;
        }
    }

    if !found {
        JNI_HOOKED.store(false, Ordering::Release);
        return false;
    }

    unsafe {
        *(env as *mut *mut c_void) = new_functions;
    }
    unsafe {
        libc::mprotect(new_functions, table_size, libc::PROT_READ);
    }

    true
}

pub fn restore_jni_env(env: *mut c_void) {
    let old = ORIG_FUNCTIONS.load(Ordering::Acquire);
    if !old.is_null() && !env.is_null() {
        unsafe { *(env as *mut *mut c_void) = old; }
    }
}
