use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub const MOD_DRIVER: &str = "ferens_irq";
pub const MOD_NODE: &str = "/dev/ferens_irq0";
pub const IOCTL_WAIT: u32 = 0x4008_6601;
pub const IOCTL_COUNT: u32 = 0x8008_6602;
pub const IOCTL_INFO: u32 = 0x8018_6603;
pub const IOCTL_RESET: u32 = 0x0000_6604;

pub trait IrqWait {
    fn wait_interrupt(&self, timeout: Duration) -> io::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ModInfo {
    pub count: u64,
    pub irq: u32,
    pub msi: u32,
    pub vendor: u16,
    pub device: u16,
    pub pad: [u16; 2],
}

pub struct FerensMod {
    file: File,
}

impl FerensMod {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(MOD_NODE)?;
        Ok(Self { file })
    }

    pub fn info(&self) -> io::Result<ModInfo> {
        let mut info = ModInfo {
            count: 0,
            irq: 0,
            msi: 0,
            vendor: 0,
            device: 0,
            pad: [0; 2],
        };
        let status = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                IOCTL_INFO as libc::c_ulong,
                &mut info as *mut ModInfo,
            )
        };
        if status < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    pub fn count(&self) -> io::Result<u64> {
        let mut count: u64 = 0;
        let status = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                IOCTL_COUNT as libc::c_ulong,
                &mut count as *mut u64,
            )
        };
        if status < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count)
    }

    pub fn reset(&self) -> io::Result<()> {
        let status = unsafe { libc::ioctl(self.file.as_raw_fd(), IOCTL_RESET as libc::c_ulong) };
        if status < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl IrqWait for FerensMod {
    fn wait_interrupt(&self, timeout: Duration) -> io::Result<()> {
        let mut millis = timeout.as_millis() as u64;
        if millis == 0 {
            millis = 1;
        }
        let status = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                IOCTL_WAIT as libc::c_ulong,
                &millis as *const u64,
            )
        };
        if status < 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ETIMEDOUT) => Err(io::Error::other(format!(
                    "no interrupt after {}ms (module reported timeout)",
                    millis
                ))),
                Some(libc::EINTR) => Err(io::Error::other("interrupt wait was signalled")),
                _ => Err(error),
            };
        }
        Ok(())
    }
}

pub fn module_loaded() -> bool {
    Path::new("/sys/module").join(MOD_DRIVER).exists()
}

fn ensure_node() -> io::Result<()> {
    if Path::new(MOD_NODE).exists() {
        return Ok(());
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        if Path::new(MOD_NODE).exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let dev_attr = Path::new("/sys/class")
        .join(MOD_DRIVER)
        .join(MOD_NODE)
        .join("dev");
    if !dev_attr.exists() {
        return Err(io::Error::other(format!(
            "{MOD_NODE} did not appear and no sysfs class device exists"
        )));
    }
    let text = fs::read_to_string(&dev_attr)?;
    let (major, minor) = text
        .trim()
        .split_once(':')
        .ok_or_else(|| io::Error::other(format!("unexpected dev attr {text:?}")))?;
    let major: u32 = major
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let minor: u32 = minor
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mode = libc::S_IFCHR | 0o600;
    let status = unsafe {
        libc::mknod(
            MOD_NODE.as_ptr().cast::<libc::c_char>(),
            mode,
            libc::makedev(major, minor),
        )
    };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn ensure_module_irq(slot_dir: &Path, slot: &str) -> io::Result<FerensMod> {
    if Path::new(MOD_NODE).exists() {
        return FerensMod::open();
    }
    if !module_loaded() {
        let mut inserted = false;
        for candidate in ["kernel/ferens_irq/ferens_irq.ko", "ferens_irq.ko"] {
            if !Path::new(candidate).exists() {
                continue;
            }
            let status = Command::new("insmod").arg(candidate).status()?;
            if !status.success() {
                return Err(io::Error::other(format!("insmod {candidate} failed")));
            }
            inserted = true;
            break;
        }
        if !inserted {
            return Err(io::Error::other(
                "module ferens_irq is not loaded; build it with `make -C kernel/ferens_irq` and insmod ferens_irq.ko",
            ));
        }
    }
    let driver_dir = Path::new("/sys/bus/pci/drivers").join(MOD_DRIVER);
    if !driver_dir.exists() {
        return Err(io::Error::other(format!(
            "{MOD_DRIVER} loaded but its pci driver directory is missing"
        )));
    }
    let bound = fs::read_link(slot_dir.join("driver")).ok();
    let bound_name = bound.as_ref().map(|target| {
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    if bound_name.as_deref() != Some(MOD_DRIVER) {
        fs::write(driver_dir.join("bind"), slot)?;
    }
    ensure_node()?;
    FerensMod::open()
}

pub fn release_module_irq(slot_dir: &Path, slot: &str) -> io::Result<()> {
    let bound = fs::read_link(slot_dir.join("driver")).ok();
    let bound_name = bound.as_ref().map(|target| {
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    if bound_name.as_deref() != Some(MOD_DRIVER) {
        return Ok(());
    }
    let target = Path::new("/sys/bus/pci/drivers")
        .join(MOD_DRIVER)
        .join("unbind");
    fs::write(target, slot)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn ioctl_numbers_match_kernel_macros() {
        assert_eq!(IOCTL_WAIT, 0x4008_6601);
        assert_eq!(IOCTL_COUNT, 0x8008_6602);
        assert_eq!(IOCTL_INFO, 0x8018_6603);
        assert_eq!(IOCTL_RESET, 0x0000_6604);
    }

    #[test]
    fn mod_info_layout_matches_kernel_struct() {
        assert_eq!(size_of::<ModInfo>(), 24);
        assert_eq!(offset_of!(ModInfo, irq), 8);
        assert_eq!(offset_of!(ModInfo, msi), 12);
        assert_eq!(offset_of!(ModInfo, vendor), 16);
        assert_eq!(offset_of!(ModInfo, device), 18);
        assert_eq!(offset_of!(ModInfo, pad), 20);
    }

    struct CountingIrq {
        waits: std::cell::Cell<usize>,
    }

    impl IrqWait for CountingIrq {
        fn wait_interrupt(&self, _timeout: Duration) -> io::Result<()> {
            self.waits.set(self.waits.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn counting_irq_records_waits() {
        let irq = CountingIrq {
            waits: std::cell::Cell::new(0),
        };
        irq.wait_interrupt(Duration::from_millis(10)).unwrap();
        irq.wait_interrupt(Duration::from_secs(1)).unwrap();
        assert_eq!(irq.waits.get(), 2);
    }
}
