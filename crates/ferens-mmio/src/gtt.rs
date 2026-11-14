use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;

pub const PAGEMAP_PATH: &str = "/proc/self/pagemap";
pub const PAGEMAP_PRESENT: u64 = 1 << 63;
pub const PAGEMAP_PFN_MASK: u64 = 0x007F_FFFF_FFFF_FFFF;
pub const PAGEMAP_ENTRY_BYTES: usize = 8;
pub const GPU_PAGE_BYTES: usize = 4096;

pub fn parse_pagemap_entry(entry: u64) -> Option<u64> {
    if entry & PAGEMAP_PRESENT == 0 {
        None
    } else {
        Some(entry & PAGEMAP_PFN_MASK)
    }
}

pub fn pagemap_offset(virtual_addr: usize) -> i64 {
    (virtual_addr / GPU_PAGE_BYTES * PAGEMAP_ENTRY_BYTES) as i64
}

pub struct HostBuffer {
    ptr: *mut u8,
    pages: usize,
    pfns: Vec<u64>,
}

unsafe impl Send for HostBuffer {}
unsafe impl Sync for HostBuffer {}

impl HostBuffer {
    pub fn lock(pages: usize) -> io::Result<Self> {
        if pages == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty gtt allocation",
            ));
        }
        let bytes = pages * GPU_PAGE_BYTES;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_LOCKED | libc::MAP_POPULATE,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::mlock(ptr, bytes) } != 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::munmap(ptr, bytes) };
            return Err(error);
        }
        let pfns = match Self::read_pfns(ptr as usize, pages) {
            Ok(pfns) => pfns,
            Err(error) => {
                unsafe {
                    libc::munlock(ptr, bytes);
                    libc::munmap(ptr, bytes);
                }
                return Err(error);
            }
        };
        Ok(Self {
            ptr: ptr.cast::<u8>(),
            pages,
            pfns,
        })
    }

    fn read_pfns(virtual_addr: usize, pages: usize) -> io::Result<Vec<u64>> {
        let file = OpenOptions::new()
            .read(true)
            .open(PAGEMAP_PATH)
            .map_err(|error| io::Error::other(format!("{}: {error}", PAGEMAP_PATH)))?;
        let mut entries = vec![0u64; pages];
        let wanted = pages * PAGEMAP_ENTRY_BYTES;
        let read = unsafe {
            libc::pread(
                file.as_raw_fd(),
                entries.as_mut_ptr().cast(),
                wanted,
                pagemap_offset(virtual_addr),
            )
        };
        if read != wanted as isize {
            return Err(io::Error::other("pagemap read is short"));
        }
        entries
            .iter()
            .map(|&entry| {
                parse_pagemap_entry(entry)
                    .ok_or_else(|| io::Error::other("page is not resident, pagemap needs root"))
            })
            .collect()
    }

    pub fn pfns(&self) -> &[u64] {
        &self.pfns
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    pub fn write_u32(&self, byte_offset: usize, value: u32) {
        assert!(
            byte_offset.is_multiple_of(4) && byte_offset + 4 <= self.pages * GPU_PAGE_BYTES,
            "offset out of range"
        );
        unsafe { std::ptr::write_volatile(self.ptr.add(byte_offset).cast::<u32>(), value) }
    }

    pub fn read_u32(&self, byte_offset: usize) -> u32 {
        assert!(
            byte_offset.is_multiple_of(4) && byte_offset + 4 <= self.pages * GPU_PAGE_BYTES,
            "offset out of range"
        );
        unsafe { std::ptr::read_volatile(self.ptr.add(byte_offset).cast::<u32>()) }
    }
}

impl Drop for HostBuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munlock(self.ptr.cast::<libc::c_void>(), self.pages * GPU_PAGE_BYTES);
            libc::munmap(self.ptr.cast::<libc::c_void>(), self.pages * GPU_PAGE_BYTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagemap_entry_yields_pfn_only_when_present() {
        assert_eq!(parse_pagemap_entry(0x8000_0000_0000_1234), Some(0x1234));
        assert_eq!(parse_pagemap_entry(0x0000_0000_0000_1234), None);
        assert_eq!(parse_pagemap_entry(0), None);
        assert_eq!(parse_pagemap_entry(PAGEMAP_PRESENT), Some(0));
        assert_eq!(PAGEMAP_PFN_MASK, 0x007F_FFFF_FFFF_FFFF);
    }

    #[test]
    fn pagemap_offset_scales_with_entries() {
        assert_eq!(pagemap_offset(0), 0);
        assert_eq!(pagemap_offset(GPU_PAGE_BYTES), 8);
        assert_eq!(pagemap_offset(3 * GPU_PAGE_BYTES + 128), 24);
        assert_eq!(
            pagemap_offset(0x0000_7F3A_BCDE_5000),
            (0x0000_7F3A_BCDE_5000usize / 4096 * 8) as i64
        );
    }
}
