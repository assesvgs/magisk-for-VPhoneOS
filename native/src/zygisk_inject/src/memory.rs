use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;

struct MmapAllocator;

unsafe impl GlobalAlloc for MmapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let real_size = layout.size().max(16).next_power_of_two();
        let ptr = libc::mmap(
            core::ptr::null_mut(),
            real_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if ptr == libc::MAP_FAILED {
            core::ptr::null_mut()
        } else {
            ptr as *mut u8
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let real_size = layout.size().max(16).next_power_of_two();
        libc::munmap(ptr as *mut c_void, real_size);
    }
}

#[global_allocator]
static ALLOCATOR: MmapAllocator = MmapAllocator;
