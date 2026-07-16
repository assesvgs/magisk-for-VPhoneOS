use core::ffi::c_void;
use alloc::vec::Vec;

pub struct ZygiskModule {
    pub handle: *mut c_void,
    pub api_handle: *mut c_void,
    pub id: alloc::string::String,
}

type ModuleEntry = unsafe extern "C" fn(
    api: *mut crate::module_api::ZygiskModuleApi,
    env: *mut c_void,
) -> i32;

impl ZygiskModule {
    pub fn load(
        fd: i32,
        module_id: &str,
    ) -> Option<Self> {
        // 优先通过 fd 加载（防 TOCTOU 篡改），失败则降级到文件路径加载。
        // 双路径兼容真机和 VPhoneOS，无需平台检测。
        let handle = if fd > 0 {
            let mut path = alloc::vec::Vec::from(b"/proc/self/fd/" as &[u8]);
            let mut n = fd;
            let mut digits = alloc::vec::Vec::with_capacity(12);
            if n == 0 {
                digits.push(b'0');
            } else {
                while n > 0 {
                    digits.push(b'0' + (n % 10) as u8);
                    n /= 10;
                }
                digits.reverse();
            }
            path.extend(digits);
            path.push(0);
            let c_path = unsafe { core::ffi::CStr::from_bytes_with_nul_unchecked(&path) };
            unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW) }
        } else {
            core::ptr::null_mut()
        };

        // fd 加载失败→降级到文件路径
        let handle = if handle.is_null() {
            let mut so_path = alloc::string::String::from("/data/adb/modules/");
            so_path.push_str(module_id);
            so_path.push_str("/zygisk/zygisk.so");
            let c_path = alloc::ffi::CString::new(so_path).ok()?;
            unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW) }
        } else {
            handle
        };

        if handle.is_null() { return None; }

        let entry_sym = unsafe {
            libc::dlsym(handle, b"zygisk_module_entry\0".as_ptr() as *const libc::c_char)
        };
        if entry_sym.is_null() {
            unsafe { libc::dlclose(handle); }
            return None;
        }

        let entry: ModuleEntry = unsafe { core::mem::transmute(entry_sym) };

        let mut impl_table = alloc::boxed::Box::new(crate::module_api::ZygiskModuleImpl {
            pre_app_specialize: None,
            post_app_specialize: None,
            pre_server_specialize: None,
            post_server_specialize: None,
        });

        let mut v4_table = alloc::boxed::Box::new(crate::module_api::ModuleApiV4::default());
        crate::module_api::ModuleApiV4::populate(&mut v4_table, handle);

        let api_ptr = &mut *v4_table as *mut crate::module_api::ModuleApiV4;
        let zy_api = api_ptr as *mut crate::module_api::ZygiskModuleApi;
        unsafe {
            (*zy_api).base = &mut *impl_table as *mut crate::module_api::ZygiskModuleImpl as *mut c_void;
            (*zy_api).impl_size = core::mem::size_of::<crate::module_api::ModuleApiV4>() as u32;
        }

        let jni_env = crate::jni_env::get_global_jni_env();

        let ret = unsafe { entry(zy_api, jni_env) };

        if ret != 0 {
            unsafe { libc::dlclose(handle); }
            return None;
        }

        let _ = alloc::boxed::Box::into_raw(impl_table);
        let _ = alloc::boxed::Box::into_raw(v4_table);

        Some(ZygiskModule {
            handle,
            api_handle: api_ptr as *mut c_void,
            id: alloc::string::String::from(module_id),
        })
    }

    pub fn on_load(&self, _env: *mut c_void) {}
}

pub fn load_modules(fds: &[i32]) -> Vec<ZygiskModule> {
    let mut modules = Vec::new();
    let dir = unsafe {
        libc::opendir(b"/data/adb/modules\0".as_ptr() as *const libc::c_char)
    };
    if dir.is_null() { return modules; }

    let mut idx = 0usize;
    loop {
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() { break; }
        let name = unsafe { (*entry).d_name };
        if name[0] == b'.' as libc::c_char { continue; }
        let name_slice = unsafe { core::ffi::CStr::from_ptr(name.as_ptr()) };
        if let Ok(s) = name_slice.to_str() {
            let fd = if idx < fds.len() { fds[idx] } else { -1 };
            if let Some(m) = ZygiskModule::load(fd, s) {
                modules.push(m);
            }
            idx += 1;
        }
    }
    unsafe { libc::closedir(dir); }
    modules
}

impl crate::hook_context::HookContext {
    pub fn run_modules_pre_impl(&mut self, fds: &[i32]) {
        if self.modules.is_empty() {
            self.modules = load_modules(fds);
        }
        for m in &self.modules {
            crate::module_api::call_pre_app_specialize(m.api_handle);
        }
    }

    pub fn run_modules_post_impl(&mut self) {
        for m in &self.modules {
            crate::module_api::call_post_app_specialize(m.api_handle);
        }
    }
}
