use core::alloc::{GlobalAlloc, Layout};

struct MmapAllocator;

unsafe impl GlobalAlloc for MmapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(16).next_power_of_two();
        let ptr = libc::mmap(
            core::ptr::null_mut(),
            size,
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
        libc::munmap(ptr as *mut libc::c_void, layout.size());
    }
}

#[global_allocator]
static ALLOCATOR: MmapAllocator = MmapAllocator;
