use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const UIO_DRIVER: &str = "uio_pci_generic";
pub const COMMAND_MEMORY: u16 = 0x2;
pub const COMMAND_MASTER: u16 = 0x4;
pub const COMMAND_INTX_DISABLE: u16 = 0x400;

pub fn current_driver(slot_dir: &Path) -> Option<String> {
    let target = fs::read_link(slot_dir.join("driver")).ok()?;
    target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

pub fn unbind_current(slot_dir: &Path, slot: &str) -> io::Result<Option<String>> {
    let Some(driver) = current_driver(slot_dir) else {
        return Ok(None);
    };
    if driver == UIO_DRIVER {
        return Ok(Some(driver));
    }
    let mut target = PathBuf::from("/sys/bus/pci/drivers");
    target.push(&driver);
    target.push("unbind");
    fs::write(&target, slot)?;
    Ok(Some(driver))
}

pub fn find_uio(slot: &str) -> Option<PathBuf> {
    let uio_root = Path::new("/sys/bus/pci/devices").join(slot).join("uio");
    let entries = fs::read_dir(&uio_root).ok()?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names.into_iter().next().map(|name| uio_root.join(name))
}

pub fn ensure_uio_bound(slot: &str) -> io::Result<PathBuf> {
    if let Some(path) = find_uio(slot) {
        return Ok(path);
    }
    if !Path::new("/sys/module").join(UIO_DRIVER).exists() {
        let status = Command::new("modprobe").arg(UIO_DRIVER).status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "modprobe {UIO_DRIVER} failed with exit code {:?}",
                status.code()
            )));
        }
    }
    let driver_dir = Path::new("/sys/bus/pci/drivers").join(UIO_DRIVER);
    if !driver_dir.exists() {
        return Err(io::Error::other(format!(
            "{UIO_DRIVER} is not loaded and modprobe could not load it"
        )));
    }
    fs::write(driver_dir.join("bind"), slot)?;
    let started = Instant::now();
    while find_uio(slot).is_none() {
        if started.elapsed() > Duration::from_secs(3) {
            return Err(io::Error::other(format!(
                "uio device for {slot} did not appear after bind"
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    find_uio(slot).ok_or_else(|| io::Error::other(format!("uio device for {slot} disappeared")))
}

pub fn unbind_uio(slot: &str) -> io::Result<()> {
    if find_uio(slot).is_none() {
        return Ok(());
    }
    let target = Path::new("/sys/bus/pci/drivers")
        .join(UIO_DRIVER)
        .join("unbind");
    fs::write(target, slot)?;
    Ok(())
}

pub fn set_command_flags(slot_dir: &Path) -> io::Result<()> {
    let mut config = File::open(slot_dir.join("config"))?;
    let mut header = [0u8; 6];
    config.read_exact(&mut header)?;
    let command = u16::from_le_bytes([header[4], header[5]]);
    let updated = (command | COMMAND_MEMORY | COMMAND_MASTER) & !COMMAND_INTX_DISABLE;
    if updated == command {
        return Ok(());
    }
    let mut config = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_SYNC)
        .open(slot_dir.join("config"))?;
    config.seek(SeekFrom::Start(4))?;
    config.write_all(&updated.to_le_bytes())?;
    Ok(())
}

pub struct UioMap {
    pub addr: u64,
    pub size: u64,
    pub offset: u64,
}

pub struct UioDevice {
    file: File,
    pub path: PathBuf,
    pub maps: Vec<UioMap>,
}

impl UioDevice {
    pub fn open(uio_dir: &Path) -> io::Result<Self> {
        let name = uio_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| io::Error::other("uio directory has no name"))?;
        let mut maps = Vec::new();
        for index in 0..8 {
            let map_dir = uio_dir.join("maps").join(format!("map{index}"));
            if !map_dir.exists() {
                break;
            }
            let addr = read_hex(&map_dir.join("addr"))?;
            let size = read_hex(&map_dir.join("size"))?;
            let offset = read_hex(&map_dir.join("offset"))?;
            maps.push(UioMap { addr, size, offset });
        }
        if maps.is_empty() {
            return Err(io::Error::other(format!("{name} exposes no bar maps")));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(Path::new("/dev").join(&name))?;
        Ok(Self {
            file,
            path: uio_dir.to_path_buf(),
            maps,
        })
    }

    pub fn wait_interrupt(&self, timeout: Duration) -> io::Result<()> {
        let started = Instant::now();
        loop {
            let mut pollfd = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let deadline = timeout.saturating_sub(started.elapsed());
            if deadline.is_zero() {
                return Err(io::Error::other(format!(
                    "no interrupt after {}ms",
                    timeout.as_millis()
                )));
            }
            let millis = deadline.as_millis().min(i32::MAX as u128) as i32;
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready == 0 {
                return Err(io::Error::other(format!(
                    "no interrupt after {}ms",
                    timeout.as_millis()
                )));
            }
            if pollfd.revents & libc::POLLIN != 0 {
                let mut count = [0u8; 4];
                let mut file = &self.file;
                file.read_exact(&mut count)?;
                return Ok(());
            }
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                return Err(io::Error::other("uio fd reported POLLERR/POLLHUP"));
            }
        }
    }
}

fn read_hex(path: &Path) -> io::Result<u64> {
    let text = fs::read_to_string(path)?;
    u64::from_str_radix(text.trim().trim_start_matches("0x"), 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_flags_set_master_and_keep_intx_enabled() {
        let command = COMMAND_MEMORY | COMMAND_INTX_DISABLE;
        let updated = (command | COMMAND_MEMORY | COMMAND_MASTER) & !COMMAND_INTX_DISABLE;
        assert_eq!(updated, COMMAND_MEMORY | COMMAND_MASTER);
        assert_eq!(updated & COMMAND_INTX_DISABLE, 0);
    }
}
