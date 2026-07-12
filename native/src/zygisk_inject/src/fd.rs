

pub const MAX_FD_SIZE: usize = 1024;

#[repr(C)]
pub struct FdSet {
    bits: [u64; MAX_FD_SIZE / 64],
}

impl FdSet {
    pub fn new() -> Self { Self { bits: [0u64; 16] } }

    pub fn add(&mut self, fd: i32) {
        if fd < 0 || fd as usize >= MAX_FD_SIZE { return; }
        let idx = fd as usize / 64;
        let bit = fd as usize % 64;
        self.bits[idx] |= 1 << bit;
    }

    pub fn contains(&self, fd: i32) -> bool {
        if fd < 0 || fd as usize >= MAX_FD_SIZE { return false; }
        let idx = fd as usize / 64;
        let bit = fd as usize % 64;
        (self.bits[idx] & (1 << bit)) != 0
    }

    pub fn remove(&mut self, fd: i32) {
        if fd < 0 || fd as usize >= MAX_FD_SIZE { return; }
        let idx = fd as usize / 64;
        let bit = fd as usize % 64;
        self.bits[idx] &= !(1 << bit);
    }

    pub fn iter(&self) -> FdIter<'_> {
        FdIter { bits: &self.bits, current: 0 }
    }

    pub fn clear(&mut self) { self.bits = [0u64; 16]; }
}

pub struct FdIter<'a> {
    bits: &'a [u64; 16],
    current: usize,
}

impl<'a> Iterator for FdIter<'a> {
    type Item = i32;
    fn next(&mut self) -> Option<i32> {
        while self.current < MAX_FD_SIZE {
            let idx = self.current / 64;
            let bit = self.current % 64;
            if (self.bits[idx] & (1 << bit)) != 0 {
                let fd = self.current as i32;
                self.current += 1;
                return Some(fd);
            }
            self.current += 1;
        }
        None
    }
}

pub fn record_open_fds(fds: &mut FdSet) {
    fds.clear();
    let dir = unsafe { libc::opendir(b"/proc/self/fd\0".as_ptr() as *const libc::c_char) };
    if dir.is_null() { return; }
    loop {
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() { break; }
        let name = unsafe { (*entry).d_name };
        let mut fd_val: i32 = 0;
        let mut valid = true;
        for &c in name.iter() {
            if c == 0 { break; }
            if (c as u8) >= b'0' && (c as u8) <= b'9' {
                fd_val = fd_val * 10 + (c as u8 - b'0') as i32;
            } else { valid = false; break; }
        }
        if valid && fd_val >= 0 && (fd_val as usize) < MAX_FD_SIZE {
            fds.add(fd_val);
        }
    }
    unsafe { libc::closedir(dir); }
}

pub fn sanitize_fds(allowed: &FdSet, exempted: &[i32]) {
    let mut actual = FdSet::new();
    record_open_fds(&mut actual);
    for fd in actual.iter() {
        if allowed.contains(fd) { continue; }
        if exempted.contains(&fd) { continue; }
        unsafe { libc::close(fd); }
    }
}
