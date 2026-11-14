use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

use crate::ioctl::{self, GetVersionArgs};

pub struct Device {
    file: File,
    pub version: (u32, u32),
}

impl Device {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(ioctl::KFD_PATH)
            .map_err(|error| io::Error::other(format!("{}: {error}", ioctl::KFD_PATH)))?;
        let mut args = GetVersionArgs::default();
        ioctl::call(file.as_raw_fd(), ioctl::GET_VERSION, &mut args)?;
        Ok(Self {
            file,
            version: (args.major, args.minor),
        })
    }

    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_open_requires_kfd_file() {
        let exists = std::path::Path::new("/dev/kfd").exists();
        let result = super::Device::open();
        assert_eq!(result.is_ok(), exists);
        if let Ok(device) = result {
            assert!(device.version.0 >= 1);
            assert!(device.fd() >= 0);
        }
    }
}
