use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;
use std::slice;

use crate::ioctl::{self, AllocMemoryOfGpuArgs, FreeMemoryOfGpuArgs, MapMemoryToGpuArgs};

pub const GPU_PAGE_BYTES: usize = 4096;

pub struct Allocator<'a> {
    kfd_fd: i32,
    render: &'a File,
}

impl<'a> Allocator<'a> {
    pub fn new(kfd_fd: i32, render: &'a File) -> Self {
        Self { kfd_fd, render }
    }

    pub fn alloc(&self, gpu_id: u32, bytes: usize, flags: u32) -> io::Result<GttAlloc> {
        if bytes == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "zero size"));
        }
        let size = align_page(bytes);
        let va = reserve(size)?;
        let mut alloc = AllocMemoryOfGpuArgs {
            va_addr: va,
            size: size as u64,
            gpu_id,
            flags,
            ..Default::default()
        };
        ioctl::call(self.kfd_fd, ioctl::ALLOC_MEMORY_OF_GPU, &mut alloc)?;
        let handle = alloc.handle;
        let mmap_offset = alloc.mmap_offset;
        let ids = [gpu_id];
        let mut map = MapMemoryToGpuArgs {
            handle,
            device_ids_array_ptr: ids.as_ptr() as u64,
            n_devices: 1,
            n_success: 0,
        };
        ioctl::call(self.kfd_fd, ioctl::MAP_MEMORY_TO_GPU, &mut map)?;
        if map.n_success != map.n_devices {
            let mut free = FreeMemoryOfGpuArgs { handle };
            let _ = ioctl::call(self.kfd_fd, ioctl::FREE_MEMORY_OF_GPU, &mut free);
            let _ = unreserve(va, size);
            return Err(io::Error::other(format!(
                "gpu map covered {} of {} devices",
                map.n_success, map.n_devices
            )));
        }
        let ptr = map_fixed(self.render.as_raw_fd(), va, size, mmap_offset)?;
        Ok(GttAlloc {
            kfd_fd: self.kfd_fd,
            va,
            size,
            handle,
            ptr,
        })
    }
}

pub fn align_page(bytes: usize) -> usize {
    (bytes + GPU_PAGE_BYTES - 1) & !(GPU_PAGE_BYTES - 1)
}

fn reserve(bytes: usize) -> io::Result<u64> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            bytes,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(ptr as usize as u64)
}

fn map_fixed(fd: i32, va: u64, bytes: usize, offset: u64) -> io::Result<*mut u8> {
    let ptr = unsafe {
        libc::mmap(
            va as usize as *mut libc::c_void,
            bytes,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_FIXED,
            fd,
            offset as libc::off_t,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(ptr.cast::<u8>())
}

fn unreserve(va: u64, bytes: usize) -> io::Result<()> {
    let rc = unsafe { libc::munmap(va as usize as *mut libc::c_void, bytes) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub struct GttAlloc {
    kfd_fd: i32,
    pub va: u64,
    pub size: usize,
    pub handle: u64,
    ptr: *mut u8,
}

unsafe impl Send for GttAlloc {}
unsafe impl Sync for GttAlloc {}

impl GttAlloc {
    pub fn words_mut(&mut self) -> &mut [u32] {
        assert!(!self.ptr.is_null() && self.size.is_multiple_of(4));
        unsafe { slice::from_raw_parts_mut(self.ptr.cast::<u32>(), self.size / 4) }
    }

    pub fn read_u32(&self, offset: usize) -> u32 {
        assert!(offset.is_multiple_of(4) && offset + 4 <= self.size);
        unsafe { std::ptr::read_volatile(self.ptr.add(offset).cast::<u32>()) }
    }

    pub fn write_u32(&mut self, offset: usize, value: u32) {
        assert!(offset.is_multiple_of(4) && offset + 4 <= self.size);
        unsafe { std::ptr::write_volatile(self.ptr.add(offset).cast::<u32>(), value) }
    }
}

impl Drop for GttAlloc {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr.cast::<libc::c_void>(), self.size) };
        let mut free = FreeMemoryOfGpuArgs {
            handle: self.handle,
        };
        let _ = ioctl::call(self.kfd_fd, ioctl::FREE_MEMORY_OF_GPU, &mut free);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_align_rounds_up() {
        assert_eq!(align_page(1), GPU_PAGE_BYTES);
        assert_eq!(align_page(GPU_PAGE_BYTES), GPU_PAGE_BYTES);
        assert_eq!(align_page(GPU_PAGE_BYTES + 1), 2 * GPU_PAGE_BYTES);
        assert_eq!(align_page(40), GPU_PAGE_BYTES);
    }
}
