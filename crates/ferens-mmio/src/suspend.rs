use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use ferens_core::{BarOps, Mmio};

use crate::gfx;
use crate::hqd;
use crate::ih;
use crate::power;
use crate::rlc;
use crate::sdma;

// Стандартный заголовок PCI config space — то, что сохраняет pci_save_state
// для type 0 заголовка (расширенное пространство на Polaris не используется
// драйвером).
pub const PCI_CONFIG_BYTES: usize = 256;

// Состояние suspend хранится в /var/lib/ferens/<slot>.bin — двоичный PCI
// config слота на момент halt'а.
pub const STATE_DIR: &str = "/var/lib/ferens";

pub fn config_path(slot: &str) -> PathBuf {
    Path::new(STATE_DIR).join(slot.replace([':', '.'], "_") + ".bin")
}

// pci_save_state: сырые 256 байт config space.
pub fn save_pci_config(slot_dir: &Path) -> io::Result<Vec<u8>> {
    let mut config = Vec::new();
    fs::File::open(slot_dir.join("config"))?
        .take(PCI_CONFIG_BYTES as u64)
        .read_to_end(&mut config)?;
    if config.len() < 16 {
        return Err(io::Error::other(format!(
            "pci config read returned only {} bytes",
            config.len()
        )));
    }
    Ok(config)
}

// pci_restore_state: BARs и заголовочные регистры восстанавливаются раньше
// командного регистра — командный (dword 1) пишется последним, dword 0
// (vendor/device) и dword 2/3 (status/class, RW1C и read-only) не трогаются.
pub fn restore_pci_config(slot_dir: &Path, saved: &[u8]) -> io::Result<()> {
    if saved.len() < PCI_CONFIG_BYTES {
        return Err(io::Error::other(format!(
            "saved pci config is {} bytes, need {PCI_CONFIG_BYTES}",
            saved.len()
        )));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(slot_dir.join("config"))?;
    for dword in (4..16).rev() {
        let offset = dword * 4;
        file.seek(SeekFrom::Start(offset as u64))?;
        file.write_all(&saved[offset..offset + 4])?;
    }
    // command register последним: memory space + bus master уже должны
    // включиться для работы BAR-mmap
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&saved[4..8])?;
    Ok(())
}

// vendor/device сохранённого снапшота против текущей карты.
pub fn config_matches(saved: &[u8], vendor_id: u16, device_id: u16) -> bool {
    saved.len() >= 4
        && u16::from_le_bytes([saved[0], saved[1]]) == vendor_id
        && u16::from_le_bytes([saved[2], saved[3]]) == device_id
}

// Suspend-путь amdgpu: глушим источники прерываний и движки, снимаем CG.
// Порядок как в suspend-цепочке amdgpu: IH → SDMA → GFX/MEC → RLC → clock gating.
pub fn halt_blocks<B: BarOps>(regs: &Mmio<B>) {
    ih::disable(regs);
    for instance in 0..sdma::INSTANCES {
        sdma::stop_engine(regs, instance);
    }
    regs.write_reg_direct(
        gfx::CP_ME_CNTL,
        gfx::CP_ME_CNTL_ME_HALT | gfx::CP_ME_CNTL_PFP_HALT | gfx::CP_ME_CNTL_CE_HALT,
    );
    regs.write_reg_direct(
        hqd::CP_MEC_CNTL,
        hqd::CP_MEC_CNTL_ME1_HALT | hqd::CP_MEC_CNTL_ME2_HALT,
    );
    rlc::stop(regs);
    power::cg_off(regs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_sanitizes_slot_address() {
        assert_eq!(
            config_path("0000:01:00.0"),
            Path::new(STATE_DIR).join("0000_01_00_0.bin")
        );
    }

    #[test]
    fn config_matches_checks_vendor_and_device() {
        let mut saved = vec![0u8; PCI_CONFIG_BYTES];
        saved[0..2].copy_from_slice(&0x1002u16.to_le_bytes());
        saved[2..4].copy_from_slice(&0x67c0u16.to_le_bytes());
        assert!(config_matches(&saved, 0x1002, 0x67c0));
        assert!(!config_matches(&saved, 0x1002, 0x67c1));
        // срез без первых четырёх байт vendor/device не совпадает
        assert!(!config_matches(&saved[8..], 0x1002, 0x67c0));
        let empty: Vec<u8> = Vec::new();
        assert!(!config_matches(&empty, 0x1002, 0x67c0));
    }
}
