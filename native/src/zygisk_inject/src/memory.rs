use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;

fn layout_size(layout: Layout) -> usize {
    layout.size().max(16).next_power_of_two()
}

struct MmapAllocator;

unsafe impl GlobalAlloc for MmapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout_size(layout);
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
        let size = layout_size(layout);
        libc::munmap(ptr as *mut c_void, size);
    }
}

#[global_allocator]
static ALLOCATOR: MmapAllocator = MmapAllocator;
