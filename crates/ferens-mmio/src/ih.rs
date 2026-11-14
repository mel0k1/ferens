use std::io;

use ferens_core::{BarOps, Mmio};

pub const IH_RB_CNTL: u32 = 0xE30;
pub const IH_RB_BASE: u32 = 0xE31;
pub const IH_RB_RPTR: u32 = 0xE32;
pub const IH_RB_WPTR: u32 = 0xE33;
pub const IH_RB_WPTR_ADDR_HI: u32 = 0xE34;
pub const IH_RB_WPTR_ADDR_LO: u32 = 0xE35;
pub const IH_DOORBELL_RPTR: u32 = 0xE42;
pub const IH_STATUS: u32 = 0xE38;
pub const INTERRUPT_CNTL: u32 = 0x151A;
pub const INTERRUPT_CNTL2: u32 = 0x151B;

pub const RB_ENABLE: u32 = 1;
pub const RB_SIZE_SHIFT: u32 = 1;
pub const WPTR_WRITEBACK_ENABLE: u32 = 1 << 8;
pub const ENABLE_INTR: u32 = 1 << 17;
pub const RPTR_REARM: u32 = 1 << 21;
pub const MC_VMID_SHIFT: u32 = 24;
pub const WPTR_OVERFLOW_CLEAR: u32 = 1 << 31;
pub const WPTR_OVERFLOW_FLAG: u32 = 1;
pub const DOORBELL_RPTR_ENABLE: u32 = 1 << 28;
pub const IH_DUMMY_RD_OVERRIDE: u32 = 1;
pub const IH_REQ_NONSNOOP_EN: u32 = 1 << 3;

pub const ENTRY_BYTES: u32 = 16;
pub const ENTRY_DWORDS: usize = 4;
pub const SRC_ID_SDMA0_TRAP: u32 = 224;
pub const SRC_ID_COMPUTE_EOP: u32 = 178;

pub struct IvEntry {
    pub src_id: u32,
    pub src_data: u32,
    pub ring_id: u32,
    pub vmid: u32,
    pub pasid: u32,
    pub raw: [u32; ENTRY_DWORDS],
}

impl IvEntry {
    pub fn parse(words: &[u32]) -> Self {
        Self {
            src_id: words[0] & 0xFF,
            src_data: words[1] & 0x0FFF_FFFF,
            ring_id: words[2] & 0xFF,
            vmid: (words[2] >> 8) & 0xFF,
            pasid: (words[2] >> 16) & 0xFFFF,
            raw: [words[0], words[1], words[2], words[3]],
        }
    }

    pub fn report(&self) -> String {
        format!(
            "src {} data {:#x} ring_id {} vmid {} pasid {}, raw {:08x} {:08x} {:08x} {:08x}",
            self.src_id,
            self.src_data,
            self.ring_id,
            self.vmid,
            self.pasid,
            self.raw[0],
            self.raw[1],
            self.raw[2],
            self.raw[3]
        )
    }
}

pub fn rb_size_field(ring_bytes: u32) -> u32 {
    (ring_bytes / 4).trailing_zeros() << RB_SIZE_SHIFT
}

pub fn ptr_mask(ring_bytes: u32) -> u32 {
    ring_bytes - 1
}

pub fn setup<B: BarOps>(
    regs: &Mmio<B>,
    ring_va: u64,
    wb_va: u64,
    dummy_va: u64,
    ring_bytes: u32,
    rearm: bool,
) {
    regs.write_reg_direct(INTERRUPT_CNTL2, (dummy_va >> 8) as u32);
    let cntl = regs.read_reg_direct(INTERRUPT_CNTL);
    regs.write_reg_direct(
        INTERRUPT_CNTL,
        cntl & !(IH_DUMMY_RD_OVERRIDE | IH_REQ_NONSNOOP_EN),
    );
    regs.write_reg_direct(IH_RB_BASE, (ring_va >> 8) as u32);
    let mut rb_cntl = WPTR_OVERFLOW_CLEAR | rb_size_field(ring_bytes) | WPTR_WRITEBACK_ENABLE;
    if rearm {
        rb_cntl |= RPTR_REARM;
    }
    regs.write_reg_direct(IH_RB_CNTL, rb_cntl);
    regs.write_reg_direct(IH_RB_WPTR_ADDR_LO, wb_va as u32);
    regs.write_reg_direct(IH_RB_WPTR_ADDR_HI, ((wb_va >> 32) as u32) & 0xFF);
    regs.write_reg_direct(IH_RB_RPTR, 0);
    regs.write_reg_direct(IH_RB_WPTR, 0);
    let doorbell = regs.read_reg_direct(IH_DOORBELL_RPTR);
    regs.write_reg_direct(IH_DOORBELL_RPTR, doorbell & !DOORBELL_RPTR_ENABLE);
}

pub fn enable<B: BarOps>(regs: &Mmio<B>) {
    let mut cntl = regs.read_reg_direct(IH_RB_CNTL);
    cntl |= RB_ENABLE | ENABLE_INTR;
    regs.write_reg_direct(IH_RB_CNTL, cntl);
}

pub fn disable<B: BarOps>(regs: &Mmio<B>) {
    let mut cntl = regs.read_reg_direct(IH_RB_CNTL);
    cntl &= !(RB_ENABLE | ENABLE_INTR);
    regs.write_reg_direct(IH_RB_CNTL, cntl);
    regs.write_reg_direct(IH_RB_RPTR, 0);
    regs.write_reg_direct(IH_RB_WPTR, 0);
}

pub fn clear_overflow<B: BarOps>(regs: &Mmio<B>) {
    let mut cntl = regs.read_reg_direct(IH_RB_CNTL);
    cntl |= WPTR_OVERFLOW_CLEAR;
    regs.write_reg_direct(IH_RB_CNTL, cntl);
}

pub fn writeback_wptr<B: BarOps>(
    regs: &Mmio<B>,
    wb_value: u32,
    ring_bytes: u32,
) -> io::Result<u32> {
    if wb_value & WPTR_OVERFLOW_FLAG == 0 {
        return Ok(wb_value & ptr_mask(ring_bytes));
    }
    let mut wptr = regs.read_reg_direct(IH_RB_WPTR);
    if wptr & WPTR_OVERFLOW_FLAG == 0 {
        return Ok(wptr & ptr_mask(ring_bytes));
    }
    wptr &= !WPTR_OVERFLOW_FLAG;
    let mask = ptr_mask(ring_bytes);
    let restart = (wptr + ENTRY_BYTES) & mask;
    regs.write_reg_direct(IH_RB_RPTR, restart);
    clear_overflow(regs);
    Ok(wptr & mask)
}

pub fn set_rptr<B: BarOps>(regs: &Mmio<B>, rptr_bytes: u32) {
    regs.write_reg_direct(IH_RB_RPTR, rptr_bytes);
}

pub fn drain(ring: &[u32], rptr_bytes: u32, wptr_bytes: u32, ring_bytes: u32) -> Vec<IvEntry> {
    let mask = ptr_mask(ring_bytes);
    let mut rptr = rptr_bytes & mask;
    let mut entries = Vec::new();
    while rptr != wptr_bytes & mask {
        let index = (rptr / 4) as usize % (ring_bytes as usize / 4);
        if index + ENTRY_DWORDS > ring.len() {
            break;
        }
        let words: [u32; ENTRY_DWORDS] = [
            ring[index],
            ring[index + 1],
            ring[index + 2],
            ring[index + 3],
        ];
        entries.push(IvEntry::parse(&words));
        rptr = (rptr + ENTRY_BYTES) & mask;
    }
    entries
}

pub fn wait_for_wptr<B: BarOps>(
    regs: &Mmio<B>,
    aperture: &ferens_core::MappedBar,
    wb_offset: usize,
    ring_bytes: u32,
    timeout: std::time::Duration,
) -> io::Result<u32> {
    let started = std::time::Instant::now();
    loop {
        let wb = aperture.read_u32(wb_offset);
        if wb != 0 {
            return writeback_wptr(regs, wb, ring_bytes);
        }
        if started.elapsed() > timeout {
            return Err(io::Error::other(format!(
                "IH wptr writeback stayed zero after {}ms\nih status {:#010x}, wptr reg {:#010x}, rptr reg {:#010x}",
                started.elapsed().as_millis(),
                regs.read_reg_direct(IH_STATUS),
                regs.read_reg_direct(IH_RB_WPTR),
                regs.read_reg_direct(IH_RB_RPTR),
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferens_core::BarOps;
    use std::cell::RefCell;

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
    }

    impl FakeBar {
        fn new() -> Self {
            Self {
                cells: RefCell::new(vec![0; 0x20000]),
                writes: RefCell::new(Vec::new()),
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset / 4, value));
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x20000
        }
    }

    fn entry(src_id: u32, data: u32, ring_id: u32, vmid: u32, pasid: u32) -> [u32; 4] {
        [src_id, data, ring_id | (vmid << 8) | (pasid << 16), 0x1234]
    }

    #[test]
    fn rb_size_and_mask_match_kernel_order_base_2() {
        assert_eq!(rb_size_field(4096), 10 << RB_SIZE_SHIFT);
        assert_eq!(rb_size_field(65536), 14 << RB_SIZE_SHIFT);
        assert_eq!(ptr_mask(4096), 4095);
    }

    #[test]
    fn setup_writes_base_cntl_wptr_addr_and_disables_doorbell() {
        let mmio = Mmio::new(FakeBar::new());
        mmio.bar().cells.borrow_mut()[INTERRUPT_CNTL as usize] =
            IH_DUMMY_RD_OVERRIDE | IH_REQ_NONSNOOP_EN;
        mmio.bar().cells.borrow_mut()[IH_DOORBELL_RPTR as usize] = DOORBELL_RPTR_ENABLE;
        setup(&mmio, 0x0403_D000, 0x0403_E000, 0x0402_C000, 4096, false);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[INTERRUPT_CNTL2 as usize], 0x0402_C000 >> 8);
        assert_eq!(
            cells[INTERRUPT_CNTL as usize] & (IH_DUMMY_RD_OVERRIDE | IH_REQ_NONSNOOP_EN),
            0
        );
        assert_eq!(cells[IH_RB_BASE as usize], 0x0403_D000 >> 8);
        assert_eq!(
            cells[IH_RB_CNTL as usize],
            WPTR_OVERFLOW_CLEAR | rb_size_field(4096) | WPTR_WRITEBACK_ENABLE
        );
        assert_eq!(cells[IH_RB_WPTR_ADDR_LO as usize], 0x0403_E000);
        assert_eq!(cells[IH_RB_WPTR_ADDR_HI as usize], 0);
        assert_eq!(cells[IH_RB_RPTR as usize], 0);
        assert_eq!(cells[IH_RB_WPTR as usize], 0);
        assert_eq!(cells[IH_DOORBELL_RPTR as usize] & DOORBELL_RPTR_ENABLE, 0);
    }

    #[test]
    fn setup_rearm_sets_rptr_rearm_bit_for_msi() {
        let mmio = Mmio::new(FakeBar::new());
        setup(&mmio, 0x0403_D000, 0x0403_E000, 0x0402_C000, 4096, true);
        assert_eq!(
            mmio.bar().cells.borrow()[IH_RB_CNTL as usize],
            WPTR_OVERFLOW_CLEAR | rb_size_field(4096) | WPTR_WRITEBACK_ENABLE | RPTR_REARM
        );
    }

    #[test]
    fn enable_and_disable_drive_rb_enable_bits() {
        let mmio = Mmio::new(FakeBar::new());
        enable(&mmio);
        assert_eq!(
            mmio.bar().cells.borrow()[IH_RB_CNTL as usize],
            RB_ENABLE | ENABLE_INTR
        );
        disable(&mmio);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[IH_RB_CNTL as usize] & (RB_ENABLE | ENABLE_INTR), 0);
        assert_eq!(cells[IH_RB_RPTR as usize], 0);
        assert_eq!(cells[IH_RB_WPTR as usize], 0);
    }

    #[test]
    fn iv_entry_decodes_src_data_ring_vmid_pasid() {
        let entry = IvEntry::parse(&entry(SRC_ID_SDMA0_TRAP, 0xABCD, 0, 1, 0x1234));
        assert_eq!(entry.src_id, 224);
        assert_eq!(entry.src_data, 0xABCD);
        assert_eq!(entry.ring_id, 0);
        assert_eq!(entry.vmid, 1);
        assert_eq!(entry.pasid, 0x1234);
        assert!(entry.report().contains("src 224"));
    }

    #[test]
    fn drain_walks_entries_and_wraps_by_bytes() {
        let mut ring = vec![0u32; 4096 / 4];
        ring[0..4].copy_from_slice(&entry(224, 0x1, 0, 0, 0));
        ring[4..8].copy_from_slice(&entry(178, 0x2, 0, 0, 0));
        let entries = drain(&ring, 0, 32, 4096);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].src_id, 224);
        assert_eq!(entries[1].src_id, 178);
        let last_words = 4096 / 4 - 4;
        ring[last_words..last_words + 4].copy_from_slice(&entry(224, 0x3, 0, 0, 0));
        ring[0..4].copy_from_slice(&entry(178, 0x4, 0, 0, 0));
        let wrapped = drain(&ring, 4096 - 16, 16, 4096);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].src_id, 224);
        assert_eq!(wrapped[0].src_data, 0x3);
        assert_eq!(wrapped[1].src_id, 178);
        assert_eq!(wrapped[1].src_data, 0x4);
    }

    #[test]
    fn drain_stops_on_equal_pointers() {
        let ring = vec![0u32; 16];
        assert!(drain(&ring, 0, 0, 4096).is_empty());
    }

    #[test]
    fn writeback_wptr_without_overflow_passes_through() {
        let mmio = Mmio::new(FakeBar::new());
        assert_eq!(writeback_wptr(&mmio, 0x40, 4096).unwrap(), 0x40);
    }

    #[test]
    fn writeback_wptr_overflow_restarts_and_clears() {
        let mmio = Mmio::new(FakeBar::new());
        mmio.bar().cells.borrow_mut()[IH_RB_WPTR as usize] = 0x80 | WPTR_OVERFLOW_FLAG;
        let wptr = writeback_wptr(&mmio, WPTR_OVERFLOW_FLAG | 0x80, 4096).unwrap();
        assert_eq!(wptr, 0x80);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[IH_RB_RPTR as usize], (0x80 + 16) & 4095);
        assert_eq!(cells[IH_RB_CNTL as usize], WPTR_OVERFLOW_CLEAR);
    }
}
