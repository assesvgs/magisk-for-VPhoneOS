extern "C" {
    fn zygisk_hook_jni_env() -> bool;
}

pub fn hook_jni_env() -> bool {
    unsafe { zygisk_hook_jni_env() }
}
