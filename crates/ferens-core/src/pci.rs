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

// Детекция методов полного сброса по config space — порядок повторяет
// pci_reset_fn_methods[] из drivers/pci/pci.c: flr → af_flr → pm → bus.
// device_specific (quirk-таблица) и acpi (_RST) из config space не видны.
pub const PCI_CAP_POINTER: usize = 0x34;
pub const PCI_STATUS_CAP_LIST: u16 = 0x10;
pub const PCI_CAP_ID_PM: u8 = 0x01;
pub const PCI_CAP_ID_EXP: u8 = 0x10;
pub const PCI_CAP_ID_AF: u8 = 0x13;
pub const PCI_PM_CTRL_STATE_MASK: u16 = 0x0003;
pub const PCI_PM_CTRL_NO_SOFT_RESET: u16 = 0x0008;
pub const PCI_EXP_DEVCAP_FLR: u32 = 0x1000_0000;
pub const PCI_AF_CAP_TP: u8 = 0x01;
pub const PCI_AF_CAP_FLR: u8 = 0x02;
// страховка от зацикленной цепочки capability-указателей
const CAP_WALK_HOPS: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetCaps {
    pub flr: bool,
    pub af_flr: bool,
    // Some(true) — PM-кап есть и NO_SOFT_RESET чист, сброс через D3hot→D0
    // возможен; Some(false) — NO_SOFT_RESET стоит; None — PM-капа нет
    pub pm: Option<bool>,
}

impl ResetCaps {
    pub fn method_report(self) -> String {
        let mut parts = Vec::new();
        if self.flr {
            parts.push("flr (DEVCAP, DEVCTL BCR_FLR, задержка 100 мс)");
        }
        if self.af_flr {
            parts.push("af_flr (AF cap, AF_CTRL FLR)");
        }
        match self.pm {
            Some(true) => parts.push("pm (D3hot→D0 через PMCSR)"),
            Some(false) => parts.push("pm недоступен (PMCSR NO_SOFT_RESET)"),
            None => parts.push("pm недоступен (нет PM-капы)"),
        }
        parts.push("bus (шиный сброс родительского порта)");
        parts.join("; ")
    }
}

// Обход capability-списка: бит Cap List в статусе, указатель в 0x34,
// каждый элемент — (id в байте 0, next в байте 1), смещения маскируются на 0xfc.
pub fn cap_offsets(config: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();
    let status = match config.get(1..3) {
        Some(bytes) => u16::from_le_bytes([bytes[0], bytes[1]]),
        None => return offsets,
    };
    if status & PCI_STATUS_CAP_LIST == 0 {
        return offsets;
    }
    let Some(mut ptr) = config.get(PCI_CAP_POINTER).copied() else {
        return offsets;
    };
    for _ in 0..CAP_WALK_HOPS {
        if ptr == 0 {
            break;
        }
        let pos = ptr as usize & 0xfc;
        let Some(&next) = config.get(pos + 1) else {
            break;
        };
        offsets.push(pos);
        ptr = next;
    }
    offsets
}

pub fn detect_reset_caps(config: &[u8]) -> ResetCaps {
    let mut caps = ResetCaps {
        flr: false,
        af_flr: false,
        pm: None,
    };
    for pos in cap_offsets(config) {
        match config.get(pos) {
            Some(&PCI_CAP_ID_PM) => {
                if let Some(bytes) = config.get(pos + 2..pos + 4) {
                    // PCI_PM_CTRL = cap + 2
                    let csr = u16::from_le_bytes([bytes[0], bytes[1]]);
                    caps.pm = Some(csr & PCI_PM_CTRL_NO_SOFT_RESET == 0);
                }
            }
            Some(&PCI_CAP_ID_EXP) => {
                if let Some(bytes) = config.get(pos + 4..pos + 8) {
                    // PCI_EXP_DEVCAP = cap + 4
                    let devcap = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    caps.flr = devcap & PCI_EXP_DEVCAP_FLR != 0;
                }
            }
            Some(&PCI_CAP_ID_AF) => {
                if let Some(&af_cap) = config.get(pos + 3) {
                    // PCI_AF_CAP = cap + 3: транзакции завершены и FLR заявлен
                    caps.af_flr = af_cap & PCI_AF_CAP_TP != 0 && af_cap & PCI_AF_CAP_FLR != 0;
                }
            }
            _ => {}
        }
    }
    caps
}

pub fn scan_sysfs() -> io::Result<Vec<PciDevice>> {
    scan_dir(Path::new(SYSFS_PCI_DEVICES))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_caps() -> Vec<u8> {
        let mut config = vec![0u8; 256];
        config[0..2].copy_from_slice(&0x1002u16.to_le_bytes());
        config[2..4].copy_from_slice(&0x67c0u16.to_le_bytes());
        config[1] |= PCI_STATUS_CAP_LIST as u8;
        config[PCI_CAP_POINTER] = 0x50;
        // PM-капа на 0x50: PMCSR в D0, NO_SOFT_RESET чист
        config[0x50] = PCI_CAP_ID_PM;
        config[0x51] = 0x60;
        config[0x52] = 0x00;
        config[0x53] = 0x00;
        // PCIe-капа на 0x60 с DEVCAP.FLR
        config[0x60] = PCI_CAP_ID_EXP;
        config[0x61] = 0x70;
        config[0x64..0x68].copy_from_slice(&PCI_EXP_DEVCAP_FLR.to_le_bytes());
        // AF-капа на 0x70 с TP и FLR
        config[0x70] = PCI_CAP_ID_AF;
        config[0x71] = 0x00;
        config[0x73] = PCI_AF_CAP_TP | PCI_AF_CAP_FLR;
        config
    }

    #[test]
    fn cap_walk_finds_chained_caps() {
        let config = config_with_caps();
        assert_eq!(cap_offsets(&config), vec![0x50, 0x60, 0x70]);
    }

    #[test]
    fn detect_caps_reads_all_three_methods() {
        let caps = detect_reset_caps(&config_with_caps());
        assert!(caps.flr);
        assert!(caps.af_flr);
        assert_eq!(caps.pm, Some(true));
        let report = caps.method_report();
        assert!(report.contains("flr (DEVCAP"));
        assert!(report.contains("af_flr (AF cap"));
        assert!(report.contains("pm (D3hot→D0"));
        assert!(report.ends_with("bus (шиный сброс родительского порта)"));
    }

    #[test]
    fn no_soft_reset_disables_pm_reset() {
        let mut config = config_with_caps();
        config[0x52] = PCI_PM_CTRL_NO_SOFT_RESET as u8;
        let caps = detect_reset_caps(&config);
        assert_eq!(caps.pm, Some(false));
        assert!(caps.method_report().contains("NO_SOFT_RESET"));
    }

    #[test]
    fn af_cap_without_features_disables_af_flr() {
        let mut config = config_with_caps();
        config[0x73] = 0;
        assert!(!detect_reset_caps(&config).af_flr);
        config[0x73] = PCI_AF_CAP_TP;
        assert!(!detect_reset_caps(&config).af_flr);
        config[0x73] = PCI_AF_CAP_FLR;
        assert!(!detect_reset_caps(&config).af_flr);
    }

    #[test]
    fn missing_cap_list_bit_yields_no_caps() {
        let mut config = config_with_caps();
        config[1] &= !PCI_STATUS_CAP_LIST as u8;
        assert!(cap_offsets(&config).is_empty());
        assert_eq!(
            detect_reset_caps(&config),
            ResetCaps {
                flr: false,
                af_flr: false,
                pm: None
            }
        );
    }

    #[test]
    fn looped_cap_chain_is_bounded() {
        let mut config = config_with_caps();
        config[0x51] = 0x50; // next указывает на себя
        assert_eq!(cap_offsets(&config).len(), CAP_WALK_HOPS);
    }

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
