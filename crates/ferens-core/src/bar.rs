use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

pub trait BarOps {
    fn read_u32(&self, offset: usize) -> u32;
    fn write_u32(&self, offset: usize, value: u32);
    fn size(&self) -> usize;
}

pub struct MappedBar {
    ptr: *mut u8,
    bytes: usize,
}

unsafe impl Send for MappedBar {}
unsafe impl Sync for MappedBar {}

impl MappedBar {
    pub fn from_sysfs(slot_dir: &Path, bar_index: usize) -> io::Result<Self> {
        let bytes = bar_size(slot_dir, bar_index)?;
        let path = slot_dir.join(format!("resource{bar_index}"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_SYNC)
            .open(path)?;
        Self::mmap(file, bytes)
    }

    fn mmap(file: File, bytes: usize) -> io::Result<Self> {
        if bytes == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty bar"));
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast::<u8>(),
            bytes,
        })
    }
}

impl BarOps for MappedBar {
    fn read_u32(&self, offset: usize) -> u32 {
        assert!(
            offset.is_multiple_of(4) && offset + 4 <= self.bytes,
            "offset out of range"
        );
        unsafe { std::ptr::read_volatile(self.ptr.add(offset).cast::<u32>()) }
    }

    fn write_u32(&self, offset: usize, value: u32) {
        assert!(
            offset.is_multiple_of(4) && offset + 4 <= self.bytes,
            "offset out of range"
        );
        unsafe { std::ptr::write_volatile(self.ptr.add(offset).cast::<u32>(), value) }
    }

    fn size(&self) -> usize {
        self.bytes
    }
}

impl Drop for MappedBar {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr.cast::<libc::c_void>(), self.bytes) };
    }
}

pub(crate) fn bar_size(slot_dir: &Path, bar_index: usize) -> io::Result<usize> {
    let text = fs::read_to_string(slot_dir.join("resource"))?;
    let line = text
        .lines()
        .nth(bar_index)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing bar"))?;
    let mut parts = line.split_whitespace();
    let start = parse_u64(parts.next())?;
    let end = parse_u64(parts.next())?;
    if end < start || end == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unmapped bar"));
    }
    Ok((end - start + 1) as usize)
}

fn parse_u64(text: Option<&str>) -> io::Result<u64> {
    let body = text
        .and_then(|value| value.strip_prefix("0x"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad resource line"))?;
    u64::from_str_radix(body, 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad hex number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_file_roundtrip() {
        let path = std::env::temp_dir().join(format!("ferens-bar-map-{}", std::process::id()));
        let file = File::create(&path).unwrap();
        file.set_len(64).unwrap();
        drop(file);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bar = MappedBar::mmap(file, 64).unwrap();
        bar.write_u32(8, 0x1234_5678);
        assert_eq!(bar.read_u32(8), 0x1234_5678);
        assert_eq!(bar.size(), 64);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn bar_size_parses_sysfs_resource() {
        let dir = std::env::temp_dir().join(format!("ferens-bar-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("resource"),
            "0x00000000f7b00000 0x00000000f7bfffff 0x0000000000040200\n0x0000000000000000 0x0000000000000000 0x0000000000000000\n",
        )
        .unwrap();
        assert_eq!(bar_size(&dir, 0).unwrap(), 0x100000);
        assert!(bar_size(&dir, 1).is_err());
        assert!(bar_size(&dir, 5).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[should_panic]
    fn unaligned_offset_panics() {
        let path = std::env::temp_dir().join(format!("ferens-bar-align-{}", std::process::id()));
        let file = File::create(&path).unwrap();
        file.set_len(64).unwrap();
        drop(file);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bar = MappedBar::mmap(file, 64).unwrap();
        let _ = bar.read_u32(3);
    }
}
