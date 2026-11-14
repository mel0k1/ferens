use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{BarOps, Chip, MappedBar, Mmio, PciDevice};
use ferens_gfx8::dispatch::FENCE_MAGIC;
use ferens_gfx8::pm4::PacketBuilder;
use ferens_gfx8::ring::write_packet;

use crate::fw::{self, UcodeFirmware};
use crate::hdp;
use crate::iommu;
use crate::queue::CP_SETTLE;
use crate::queue::{
    program_vm0, taken_by_driver, GFX_LAYOUT_END, GFX_META_OFFSET, GFX_RING_BYTES, GFX_RING_OFFSET,
    VRAM_BASE,
};

pub const CP_ME_CNTL: u32 = 0x21B6;
pub const CP_ME_CNTL_CE_HALT: u32 = 0x100_0000;
pub const CP_ME_CNTL_PFP_HALT: u32 = 0x400_0000;
pub const CP_ME_CNTL_ME_HALT: u32 = 0x1000_0000;
pub const CP_PFP_UCODE_ADDR: u32 = 0xF814;
pub const CP_PFP_UCODE_DATA: u32 = 0xF815;
pub const CP_ME_RAM_RADDR: u32 = 0xF816;
pub const CP_ME_RAM_WADDR: u32 = 0xF816;
pub const CP_ME_RAM_DATA: u32 = 0xF817;
pub const CP_CE_UCODE_ADDR: u32 = 0xF818;
pub const CP_CE_UCODE_DATA: u32 = 0xF819;
pub const CP_RB0_BASE: u32 = 0x3040;
pub const CP_RB0_BASE_HI: u32 = 0x30B1;
pub const CP_RB0_CNTL: u32 = 0x3041;
pub const CP_RB0_RPTR_ADDR: u32 = 0x3043;
pub const CP_RB0_RPTR_ADDR_HI: u32 = 0x3044;
pub const CP_RB0_WPTR: u32 = 0x3045;
pub const CP_RB_VMID: u32 = 0x3051;
pub const CP_ENDIAN_SWAP: u32 = 0x3050;
pub const CP_RB_DOORBELL_CONTROL: u32 = 0x3059;
pub const CP_DEVICE_ID: u32 = 0x304B;
pub const CP_MAX_CONTEXT: u32 = 0x30AE;
pub const CP_RB0_RPTR: u32 = 0x21C0;
pub const CP_RB_WPTR_DELAY: u32 = 0x21C1;

pub const RB_BUFSZ_SHIFT: u32 = 2;
pub const RB_BUFSZ_MASK: u32 = 0x3F;
pub const RB_BLKSZ_SHIFT: u32 = 8;
pub const RB_MTYPE: u32 = 0x1_8000;
pub const RB_MIN_IB_AVAILSZ: u32 = 0xC0_0000;
pub const RB_NO_UPDATE: u32 = 0x800_0000;
pub const RB_RPTR_WR_ENA: u32 = 0x8000_0000;
pub const DOORBELL_EN: u32 = 0x4000_0000;
pub const GFX_FENCE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn gfx_ring_va() -> u64 {
    GFX_RING_OFFSET
}

pub fn gfx_fence_va() -> u64 {
    GFX_META_OFFSET
}

pub fn fence_packet(fence_va: u64) -> Vec<u32> {
    let mut builder = PacketBuilder::new();
    builder.write_data(
        ferens_gfx8::op::WRITE_DATA_DST_SEL_MEMORY,
        ferens_gfx8::op::WRITE_DATA_ENGINE_ME,
        true,
        fence_va,
        &[FENCE_MAGIC],
    );
    builder.into_words()
}

pub struct GfxStatus {
    pub rptr_dw: u32,
    pub wptr_dw: u32,
    pub me_cntl: u32,
}

impl GfxStatus {
    pub fn report(&self) -> String {
        format!(
            "gfx ring rptr {} dwords wptr {} dwords cp_me_cntl {:#010x}",
            self.rptr_dw, self.wptr_dw, self.me_cntl
        )
    }
}

pub fn load_ucode<B: BarOps>(
    regs: &Mmio<B>,
    addr_reg: u32,
    data_reg: u32,
    firmware: &UcodeFirmware,
) -> io::Result<()> {
    regs.write_reg_direct(addr_reg, 0);
    for word in &firmware.words {
        regs.write_reg_direct(data_reg, *word);
    }
    verify_ucode(regs, addr_reg, data_reg, firmware)
}

pub fn verify_ucode<B: BarOps>(
    regs: &Mmio<B>,
    addr_reg: u32,
    data_reg: u32,
    firmware: &UcodeFirmware,
) -> io::Result<()> {
    let mut readback = Vec::with_capacity(firmware.words.len());
    for index in 0..firmware.words.len() {
        regs.write_reg_direct(addr_reg, index as u32);
        readback.push(regs.read_reg_direct(data_reg));
    }
    let expected = fw::crc32(&firmware.words);
    let actual = fw::crc32(&readback);
    if actual != expected {
        return Err(io::Error::other(format!(
            "gfx ucode readback crc mismatch: want {expected:#010x}, got {actual:#010x}"
        )));
    }
    regs.write_reg_direct(addr_reg, firmware.version);
    Ok(())
}

pub fn resume_ring<B: BarOps>(regs: &Mmio<B>, ring_va: u64, wb_va: u64) {
    let ring_dw: usize = GFX_RING_BYTES / 4;
    let bufsz = (ring_dw / 2).trailing_zeros();
    regs.write_reg_direct(CP_RB_WPTR_DELAY, 0);
    regs.write_reg_direct(CP_RB_VMID, 0);
    let cntl =
        (bufsz << RB_BUFSZ_SHIFT) | ((bufsz - 2) << RB_BLKSZ_SHIFT) | RB_MTYPE | RB_MIN_IB_AVAILSZ;
    regs.write_reg_direct(CP_RB0_CNTL, cntl);
    regs.write_reg_direct(CP_RB0_CNTL, cntl | RB_RPTR_WR_ENA);
    regs.write_reg_direct(CP_RB0_WPTR, 0);
    regs.write_reg_direct(CP_RB0_RPTR_ADDR, wb_va as u32);
    regs.write_reg_direct(CP_RB0_RPTR_ADDR_HI, ((wb_va >> 32) as u32) & 0xFF);
    thread::sleep(Duration::from_millis(1));
    regs.write_reg_direct(CP_RB0_CNTL, cntl);
    regs.write_reg_direct(CP_RB0_BASE, (ring_va >> 8) as u32);
    regs.write_reg_direct(CP_RB0_BASE_HI, ((ring_va >> 8) >> 32) as u32);
    let doorbell = regs.read_reg_direct(CP_RB_DOORBELL_CONTROL);
    regs.write_reg_direct(CP_RB_DOORBELL_CONTROL, doorbell & !DOORBELL_EN);
    regs.write_reg_direct(CP_MAX_CONTEXT, 7);
    regs.write_reg_direct(CP_ENDIAN_SWAP, 0);
    regs.write_reg_direct(CP_DEVICE_ID, 1);
}

pub fn halt<B: BarOps>(regs: &Mmio<B>) {
    regs.write_reg_direct(
        CP_ME_CNTL,
        CP_ME_CNTL_ME_HALT | CP_ME_CNTL_PFP_HALT | CP_ME_CNTL_CE_HALT,
    );
}

pub fn unhalt<B: BarOps>(regs: &Mmio<B>) {
    regs.write_reg_direct(CP_ME_CNTL, 0);
}

pub struct GfxRing {
    regs: Mmio<MappedBar>,
    aperture: MappedBar,
    pub fw_versions: [u32; 3],
    ring_dw: usize,
    wp: usize,
    destroyed: bool,
}

impl GfxRing {
    pub fn open(slot_dir: &Path, chip: Chip) -> io::Result<Self> {
        taken_by_driver(slot_dir)?;
        let slot = slot_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        iommu::guard(Path::new("/"), &slot)?;
        let device = PciDevice::from_sysfs(slot_dir)?;
        let regs_info = device
            .regs_bar()
            .ok_or_else(|| io::Error::other("no register bar"))?;
        let aperture_info = device
            .aperture_bar()
            .ok_or_else(|| io::Error::other("no frame buffer aperture bar"))?;
        if aperture_info.size < GFX_LAYOUT_END {
            return Err(io::Error::other(format!(
                "aperture bar is {:#x} bytes, need at least {:#x}",
                aperture_info.size, GFX_LAYOUT_END
            )));
        }
        let regs_bar = MappedBar::from_sysfs(slot_dir, regs_info.index)?;
        let aperture = MappedBar::from_sysfs(slot_dir, aperture_info.index)?;
        let regs = Mmio::new(regs_bar);
        let firmware = fw::load_gfx(chip)?;
        let mut aperture = aperture;
        program_vm0(&regs, &mut aperture)?;
        let mut gfx = Self {
            regs,
            aperture,
            fw_versions: [
                firmware[0].version,
                firmware[1].version,
                firmware[2].version,
            ],
            ring_dw: GFX_RING_BYTES / 4,
            wp: 0,
            destroyed: false,
        };
        gfx.bring_up(&firmware)?;
        Ok(gfx)
    }

    fn bring_up(&mut self, firmware: &[UcodeFirmware; 3]) -> io::Result<()> {
        halt(&self.regs);
        thread::sleep(CP_SETTLE);
        load_ucode(
            &self.regs,
            CP_PFP_UCODE_ADDR,
            CP_PFP_UCODE_DATA,
            &firmware[0],
        )?;
        load_ucode(&self.regs, CP_CE_UCODE_ADDR, CP_CE_UCODE_DATA, &firmware[1])?;
        load_ucode(&self.regs, CP_ME_RAM_WADDR, CP_ME_RAM_DATA, &firmware[2])?;
        {
            let aperture = self.aperture.words_mut();
            let ring_first = ((gfx_ring_va() - VRAM_BASE) / 4) as usize;
            let ring_last = ring_first + GFX_RING_BYTES / 4;
            let meta_first = ((gfx_fence_va() - VRAM_BASE) / 4) as usize;
            let meta_last = meta_first + 1024;
            for word in &mut aperture[ring_first..ring_last] {
                *word = 0;
            }
            for word in &mut aperture[meta_first..meta_last] {
                *word = 0;
            }
        }
        resume_ring(&self.regs, gfx_ring_va(), gfx_fence_va() + 4);
        unhalt(&self.regs);
        Ok(())
    }

    pub fn ring_test(&mut self) -> io::Result<u32> {
        let packets = fence_packet(gfx_fence_va());
        self.aperture
            .write_u32((gfx_fence_va() - VRAM_BASE) as usize, 0);
        {
            let aperture = self.aperture.words_mut();
            let first = ((gfx_ring_va() - VRAM_BASE) / 4) as usize;
            let ring = &mut aperture[first..first + self.ring_dw];
            let next = write_packet(ring, self.wp, &packets).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "packets do not fit the gfx ring",
                )
            })?;
            self.wp = next;
        }
        hdp::flush(&self.regs);
        self.regs.write_reg_direct(CP_RB0_WPTR, self.wp as u32);
        let started = Instant::now();
        loop {
            let fence = self
                .aperture
                .read_u32((gfx_fence_va() - VRAM_BASE) as usize);
            if fence == FENCE_MAGIC {
                return Ok(fence);
            }
            if started.elapsed() > GFX_FENCE_TIMEOUT {
                let status = self.status();
                return Err(io::Error::other(format!(
                    "gfx ring fence timeout after {}ms\n{}",
                    started.elapsed().as_millis(),
                    status.report()
                )));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn status(&self) -> GfxStatus {
        GfxStatus {
            rptr_dw: self.regs.read_reg_direct(CP_RB0_RPTR),
            wptr_dw: self.regs.read_reg_direct(CP_RB0_WPTR),
            me_cntl: self.regs.read_reg_direct(CP_ME_CNTL),
        }
    }

    pub fn destroy(&mut self) -> io::Result<()> {
        halt(&self.regs);
        self.destroyed = true;
        Ok(())
    }
}

impl Drop for GfxRing {
    fn drop(&mut self) {
        if !self.destroyed {
            let _ = self.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
        sram: RefCell<Vec<u32>>,
        cursor: Cell<usize>,
        addr_cell: usize,
        data_cell: usize,
    }

    impl FakeBar {
        fn for_window(addr_reg: u32, data_reg: u32) -> Self {
            Self {
                cells: RefCell::new(vec![0; 0x40000]),
                writes: RefCell::new(Vec::new()),
                sram: RefCell::new(vec![0; 0x40000]),
                cursor: Cell::new(0),
                addr_cell: addr_reg as usize * 4,
                data_cell: data_reg as usize * 4,
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            if offset == self.data_cell {
                return self.sram.borrow()[self.cursor.get()];
            }
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
            if offset == self.addr_cell {
                self.cursor.set(value as usize);
            } else if offset == self.data_cell {
                let cursor = self.cursor.get();
                self.sram.borrow_mut()[cursor] = value;
                self.cursor.set(cursor + 1);
            }
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    fn ucode() -> UcodeFirmware {
        UcodeFirmware {
            version: 0x35,
            feature_version: 1,
            words: vec![0x1234_5678, 0x9abc_def0, 0x0f1e_2d3c],
        }
    }

    #[test]
    fn pfp_ucode_streams_and_verifies() {
        let mmio = Mmio::new(FakeBar::for_window(CP_PFP_UCODE_ADDR, CP_PFP_UCODE_DATA));
        load_ucode(&mmio, CP_PFP_UCODE_ADDR, CP_PFP_UCODE_DATA, &ucode()).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (CP_PFP_UCODE_ADDR as usize * 4, 0));
        assert_eq!(writes[1], (CP_PFP_UCODE_DATA as usize * 4, 0x1234_5678));
        assert_eq!(
            writes.last().unwrap(),
            &(CP_PFP_UCODE_ADDR as usize * 4, 0x35)
        );
    }

    #[test]
    fn me_ucode_uses_ram_window() {
        let mmio = Mmio::new(FakeBar::for_window(CP_ME_RAM_WADDR, CP_ME_RAM_DATA));
        load_ucode(&mmio, CP_ME_RAM_WADDR, CP_ME_RAM_DATA, &ucode()).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (CP_ME_RAM_WADDR as usize * 4, 0));
        assert_eq!(writes[1], (CP_ME_RAM_DATA as usize * 4, 0x1234_5678));
        assert_eq!(
            writes.last().unwrap(),
            &(CP_ME_RAM_WADDR as usize * 4, 0x35)
        );
    }

    #[test]
    fn corrupted_me_ram_fails_crc() {
        let mmio = Mmio::new(FakeBar::for_window(CP_ME_RAM_WADDR, CP_ME_RAM_DATA));
        load_ucode(&mmio, CP_ME_RAM_WADDR, CP_ME_RAM_DATA, &ucode()).unwrap();
        mmio.bar().sram.borrow_mut()[2] = 0xdead_beef;
        let error = verify_ucode(&mmio, CP_ME_RAM_WADDR, CP_ME_RAM_DATA, &ucode()).unwrap_err();
        assert!(error
            .to_string()
            .contains("gfx ucode readback crc mismatch"));
    }

    #[test]
    fn fence_packet_is_write_data_with_confirm() {
        let packet = fence_packet(0x0403_A000);
        assert_eq!(packet.len(), 5);
        assert_eq!(packet[0] >> 30, 3);
        assert_eq!((packet[0] >> 16) & 0x3FFF, 4);
        assert_eq!((packet[0] >> 8) & 0xFF, ferens_gfx8::op::WRITE_DATA as u32);
        assert_eq!(
            packet[1] & 0xFF00,
            ferens_gfx8::op::WRITE_DATA_DST_SEL_MEMORY << 8
        );
        assert_eq!(
            packet[1] & ferens_gfx8::op::WRITE_DATA_WR_CONFIRM,
            ferens_gfx8::op::WRITE_DATA_WR_CONFIRM
        );
        assert_eq!(packet[2], 0x0403_A000);
        assert_eq!(packet[4], FENCE_MAGIC);
    }

    #[test]
    fn resume_ring_sets_rb_and_keeps_doorbell_off() {
        let mmio = Mmio::new(FakeBar::for_window(CP_PFP_UCODE_ADDR, CP_PFP_UCODE_DATA));
        mmio.bar().cells.borrow_mut()[CP_RB_DOORBELL_CONTROL as usize] = DOORBELL_EN;
        resume_ring(&mmio, 0x0403_9000, 0x0403_A004);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[CP_RB_WPTR_DELAY as usize], 0);
        assert_eq!(cells[CP_RB_VMID as usize], 0);
        let ring_dw = GFX_RING_BYTES as u32 / 4;
        let bufsz = (ring_dw / 2).trailing_zeros();
        let expected = (bufsz << RB_BUFSZ_SHIFT)
            | ((bufsz - 2) << RB_BLKSZ_SHIFT)
            | RB_MTYPE
            | RB_MIN_IB_AVAILSZ;
        assert_eq!(cells[CP_RB0_CNTL as usize], expected);
        assert_eq!(cells[CP_RB0_BASE as usize], 0x0403_9000 >> 8);
        assert_eq!(cells[CP_RB0_BASE_HI as usize], 0);
        assert_eq!(cells[CP_RB0_RPTR_ADDR as usize], 0x0403_A004);
        assert_eq!(cells[CP_RB0_RPTR_ADDR_HI as usize], 0);
        assert_eq!(cells[CP_RB_DOORBELL_CONTROL as usize] & DOORBELL_EN, 0);
        assert_eq!(cells[CP_MAX_CONTEXT as usize], 7);
        assert_eq!(cells[CP_ENDIAN_SWAP as usize], 0);
        assert_eq!(cells[CP_DEVICE_ID as usize], 1);
    }

    #[test]
    fn halt_and_unhalt_drive_cp_me_cntl() {
        let mmio = Mmio::new(FakeBar::for_window(CP_PFP_UCODE_ADDR, CP_PFP_UCODE_DATA));
        halt(&mmio);
        assert_eq!(
            mmio.bar().cells.borrow()[CP_ME_CNTL as usize],
            CP_ME_CNTL_ME_HALT | CP_ME_CNTL_PFP_HALT | CP_ME_CNTL_CE_HALT
        );
        unhalt(&mmio);
        assert_eq!(mmio.bar().cells.borrow()[CP_ME_CNTL as usize], 0);
    }
}
