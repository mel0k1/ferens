use std::fs;
use std::io;
use std::path::Path;

pub const SYSFS_PCI_DEVICES: &str = "/sys/bus/pci/devices";

pub const PCI_VENDOR_AMD: u16 = 0x1002;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Polaris10,
    Polaris11,
    Polaris12,
    VegaM,
    Unknown(u16),
}

impl Chip {
    pub fn from_device_id(device_id: u16) -> Self {
        match device_id {
            0x67C0 | 0x67C1 | 0x67C2 | 0x67C4 | 0x67C7 | 0x67C8 | 0x67C9 | 0x67CA | 0x67CC
            | 0x67CF | 0x67D0 | 0x67D4 | 0x67DF | 0x6FDF => Self::Polaris10,
            0x67E0 | 0x67E1 | 0x67E3 | 0x67E7 | 0x67E8 | 0x67E9 | 0x67EB | 0x67EF | 0x67FF => {
                Self::Polaris11
            }
            0x6980 | 0x6981 | 0x6985 | 0x6986 | 0x6987 | 0x698F | 0x6995 | 0x6997 | 0x699F => {
                Self::Polaris12
            }
            0x694C | 0x694E | 0x694F => Self::VegaM,
            _ => Self::Unknown(device_id),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Polaris10 => "Polaris 10 (Ellesmere)",
            Self::Polaris11 => "Polaris 11 (Baffin)",
            Self::Polaris12 => "Polaris 12 (Lexa)",
            Self::VegaM => "Vega 12 (VegaM)",
            Self::Unknown(_) => "unknown AMD chip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BarInfo {
    pub index: usize,
    pub start: u64,
    pub size: u64,
    pub memory: bool,
}

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub slot: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: u8,
    pub chip: Chip,
    pub bars: Vec<BarInfo>,
}

impl PciDevice {
    pub fn from_sysfs(dir: &Path) -> io::Result<Self> {
        let vendor_id = read_hex_file(dir, "vendor")?;
        if vendor_id != u64::from(PCI_VENDOR_AMD) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device is not AMD",
            ));
        }
        let device_id = read_hex_file(dir, "device")?;
        let revision = fs::read_to_string(dir.join("revision"))
            .ok()
            .and_then(|text| parse_hex(&text))
            .unwrap_or(0);
        let resource_text = fs::read_to_string(dir.join("resource")).unwrap_or_default();
        Ok(PciDevice {
            slot: dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            vendor_id: PCI_VENDOR_AMD,
            device_id: device_id as u16,
            revision: revision as u8,
            chip: Chip::from_device_id(device_id as u16),
            bars: parse_resource(&resource_text),
        })
    }

    pub fn regs_bar(&self) -> Option<&BarInfo> {
        self.bars
            .iter()
            .filter(|bar| bar.memory)
            .min_by_key(|bar| bar.size)
    }

    pub fn aperture_bar(&self) -> Option<&BarInfo> {
        self.bars
            .iter()
            .filter(|bar| bar.memory)
            .max_by_key(|bar| bar.size)
    }
}

fn parse_hex(text: &str) -> Option<u64> {
    let body = text.trim().strip_prefix("0x")?;
    u64::from_str_radix(body, 16).ok()
}

fn read_hex_file(dir: &Path, name: &str) -> io::Result<u64> {
    let text = fs::read_to_string(dir.join(name))?;
    parse_hex(&text).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad hex value"))
}

pub fn parse_resource(text: &str) -> Vec<BarInfo> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let mut parts = line.split_whitespace();
            let start = parse_hex(parts.next()?)?;
            let end = parse_hex(parts.next()?)?;
            let flags = parse_hex(parts.next()?)?;
            if start == 0 && end == 0 {
                return None;
            }
            if end < start {
                return None;
            }
            Some(BarInfo {
                index,
                start,
                size: end - start + 1,
                memory: flags & 0x200 != 0,
            })
        })
        .collect()
}

pub fn scan_dir(dir: &Path) -> io::Result<Vec<PciDevice>> {
    let mut devices = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if read_hex_file(&path, "vendor")? != u64::from(PCI_VENDOR_AMD) {
            continue;
        }
        devices.push(PciDevice::from_sysfs(&path)?);
    }
    devices.sort_by(|a, b| a.slot.cmp(&b.slot));
    Ok(devices)
}

pub fn scan_sysfs() -> io::Result<Vec<PciDevice>> {
    scan_dir(Path::new(SYSFS_PCI_DEVICES))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RESOURCE: &str = "0x00000000f7b00000 0x00000000f7bfffff 0x0000000000040200\n0x00000000c0000000 0x00000000cfffffff 0x0000000000040200\n0x0000000000000000 0x0000000000000000 0x0000000000000000\n0x000000000000e000 0x000000000000e0ff 0x0000000000040101\n";

    #[test]
    fn resource_parse_skips_unmapped_and_flags_memory() {
        let bars = parse_resource(SAMPLE_RESOURCE);
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].index, 0);
        assert_eq!(bars[0].size, 0x100000);
        assert!(bars[0].memory);
        assert_eq!(bars[1].size, 0x10000000);
        assert!(!bars[2].memory);
        assert_eq!(bars[2].index, 3);
    }

    #[test]
    fn chip_matches_kernel_device_ids() {
        assert_eq!(Chip::from_device_id(0x67DF), Chip::Polaris10);
        assert_eq!(Chip::from_device_id(0x699F), Chip::Polaris12);
        assert_eq!(Chip::from_device_id(0x67FF), Chip::Polaris11);
        assert_eq!(Chip::from_device_id(0x694F), Chip::VegaM);
        assert_eq!(Chip::from_device_id(0x1234), Chip::Unknown(0x1234));
    }

    #[test]
    fn scan_dir_keeps_only_amd_devices() {
        let root = std::env::temp_dir().join(format!("ferens-scan-test-{}", std::process::id()));
        let amd = root.join("0000:01:00.0");
        let other = root.join("0000:02:00.0");
        fs::create_dir_all(&amd).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(amd.join("vendor"), "0x1002\n").unwrap();
        fs::write(amd.join("device"), "0x67df\n").unwrap();
        fs::write(amd.join("revision"), "0xe7\n").unwrap();
        fs::write(amd.join("resource"), SAMPLE_RESOURCE).unwrap();
        fs::write(other.join("vendor"), "0x8086\n").unwrap();
        fs::write(other.join("device"), "0x1234\n").unwrap();
        let devices = scan_dir(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].slot, "0000:01:00.0");
        assert_eq!(devices[0].chip, Chip::Polaris10);
        assert_eq!(devices[0].revision, 0xe7);
        assert_eq!(devices[0].regs_bar().unwrap().size, 0x100000);
        assert_eq!(devices[0].aperture_bar().unwrap().size, 0x10000000);
    }

    #[test]
    fn from_sysfs_rejects_non_amd_vendor() {
        let dir = std::env::temp_dir().join(format!("ferens-vendor-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("vendor"), "0x8086\n").unwrap();
        fs::write(dir.join("device"), "0x1234\n").unwrap();
        assert!(PciDevice::from_sysfs(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
