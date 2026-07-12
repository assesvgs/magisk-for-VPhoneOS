use core::ffi::c_void;

const MODULE_ROOT: &[u8] = b"/data/adb/modules\0";
const ZYGISK_DIR: &[u8] = b"/zygisk\0";

#[repr(C)]
pub struct ZygiskModuleApi {
    pub base: *mut c_void,
    pub impl_size: u32,
    pub module: *mut c_void,
}

type ModuleEntry =
    unsafe extern "C" fn(*mut c_void) -> *mut c_void;

pub struct LoadedModule {
    pub handle: *mut c_void,
    pub api: *mut c_void,
}

pub fn load_modules() -> alloc::vec::Vec<LoadedModule> {
    let mut modules = alloc::vec::Vec::new();
    let dir = unsafe { libc::opendir(MODULE_ROOT.as_ptr() as *const libc::c_char) };
    if dir.is_null() {
        return modules;
    }

    loop {
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { (*entry).d_name };
        if name[0] == b'.' as libc::c_char {
            continue;
        }
        let name_slice = unsafe {
            core::ffi::CStr::from_ptr(name.as_ptr())
        };
        let name_str = match name_slice.to_str() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut so_path = alloc::string::String::from("/data/adb/modules/");
        so_path.push_str(name_str);
        so_path.push_str("/zygisk/zygisk.so");

        let so_c = match alloc::vec::Vec::from(so_path.as_bytes()) {
            mut v => {
                v.push(0);
                v
            }
        };

        let handle = unsafe {
            libc::dlopen(so_c.as_ptr() as *const libc::c_char, libc::RTLD_NOW)
        };
        if handle.is_null() {
            continue;
        }

        let entry_fn = unsafe {
            libc::dlsym(handle, b"zygisk_module_entry\0".as_ptr() as *const libc::c_char)
        };
        if entry_fn.is_null() {
            unsafe { libc::dlclose(handle); }
            continue;
        }

        let entry: ModuleEntry = unsafe { core::mem::transmute(entry_fn) };
        let api = unsafe { entry(core::ptr::null_mut()) };
        if api.is_null() {
            unsafe { libc::dlclose(handle); }
            continue;
        }

        modules.push(LoadedModule { handle, api });
    }

    unsafe { libc::closedir(dir); }
    modules
}
