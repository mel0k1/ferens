use ferens_core::{BarOps, Mmio};

pub const VM_CONTEXT0_PROTECTION_FAULT_STATUS: u32 = 0x536;
pub const VM_CONTEXT1_PROTECTION_FAULT_STATUS: u32 = 0x537;
pub const VM_CONTEXT0_PROTECTION_FAULT_MCCLIENT: u32 = 0x538;
pub const VM_CONTEXT1_PROTECTION_FAULT_MCCLIENT: u32 = 0x539;
pub const VM_CONTEXT0_PROTECTION_FAULT_ADDR: u32 = 0x53e;
pub const VM_CONTEXT1_PROTECTION_FAULT_ADDR: u32 = 0x53f;
pub const VM_CONTEXT1_CNTL2: u32 = 0x50d;
pub const GRBM_STATUS: u32 = 0x2004;
pub const SRBM_STATUS: u32 = 0x394;
pub const SRBM_STATUS2: u32 = 0x393;

pub const FAULT_PROTECTIONS_MASK: u32 = 0xff;
pub const FAULT_MEMORY_CLIENT_ID_MASK: u32 = 0x1ff_000;
pub const FAULT_MEMORY_CLIENT_ID_SHIFT: u32 = 0xc;
pub const FAULT_MEMORY_CLIENT_RW_MASK: u32 = 0x100_0000;
pub const FAULT_VMID_MASK: u32 = 0x1e00_0000;
pub const FAULT_VMID_SHIFT: u32 = 0x19;

pub const GRBM_GUI_ACTIVE: u32 = 1 << 31;
pub const SRBM_VMC_BUSY: u32 = 1 << 8;
pub const SRBM_MCB_BUSY: u32 = 1 << 9;
pub const SRBM_SEM_BUSY: u32 = 1 << 14;
pub const SRBM_IH_BUSY: u32 = 1 << 17;
pub const SRBM_GCATCL2_BUSY: u32 = 1 << 21;
pub const SRBM_BIF_BUSY: u32 = 1 << 29;
pub const SRBM_GRBM_RQ_PENDING: u32 = 1 << 5;
pub const SRBM2_SDMA_BUSY: u32 = 1 << 5;
pub const SRBM2_SDMA1_BUSY: u32 = 1 << 6;

pub struct FaultInfo {
    pub context: u32,
    pub protections: u32,
    pub memory_client_id: u32,
    pub write: bool,
    pub vmid: u32,
    pub page: u32,
    pub mcclient: u32,
}

pub struct Status {
    pub grbm: u32,
    pub srbm: u32,
    pub srbm2: u32,
    pub faults: Vec<FaultInfo>,
}

pub fn read_fault<B: BarOps>(mmio: &Mmio<B>, context: u32) -> Option<FaultInfo> {
    let status = mmio.read_reg_direct(VM_CONTEXT0_PROTECTION_FAULT_STATUS + context);
    let mcclient = mmio.read_reg_direct(VM_CONTEXT0_PROTECTION_FAULT_MCCLIENT + context);
    let page = mmio.read_reg_direct(VM_CONTEXT0_PROTECTION_FAULT_ADDR + context);
    if status == 0 && page == 0 {
        return None;
    }
    Some(FaultInfo {
        context,
        protections: status & FAULT_PROTECTIONS_MASK,
        memory_client_id: (status & FAULT_MEMORY_CLIENT_ID_MASK) >> FAULT_MEMORY_CLIENT_ID_SHIFT,
        write: status & FAULT_MEMORY_CLIENT_RW_MASK != 0,
        vmid: (status & FAULT_VMID_MASK) >> FAULT_VMID_SHIFT,
        page,
        mcclient,
    })
}

pub fn clear_fault<B: BarOps>(mmio: &Mmio<B>, context: u32) {
    let cntl2 = crate::vm::VM_CONTEXT0_CNTL2 + context;
    let value = mmio.read_reg_direct(cntl2);
    mmio.write_reg_direct(cntl2, value | 1);
}

pub fn snapshot<B: BarOps>(mmio: &Mmio<B>) -> Status {
    let grbm = mmio.read_reg_direct(GRBM_STATUS);
    let srbm = mmio.read_reg_direct(SRBM_STATUS);
    let srbm2 = mmio.read_reg_direct(SRBM_STATUS2);
    let faults = [0, 1]
        .iter()
        .filter_map(|context| read_fault(mmio, *context))
        .collect();
    Status {
        grbm,
        srbm,
        srbm2,
        faults,
    }
}

fn busy_bits(value: u32, bits: &[(&str, u32)]) -> String {
    bits.iter()
        .filter(|entry| value & entry.1 != 0)
        .map(|entry| entry.0)
        .collect::<Vec<_>>()
        .join(",")
}

impl Status {
    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "grbm_status {:#010x} gui_active {}",
            self.grbm,
            self.grbm & GRBM_GUI_ACTIVE != 0
        ));
        lines.push(format!(
            "srbm_status {:#010x} busy [{}]",
            self.srbm,
            busy_bits(
                self.srbm,
                &[
                    ("vmc", SRBM_VMC_BUSY),
                    ("mcb", SRBM_MCB_BUSY),
                    ("sem", SRBM_SEM_BUSY),
                    ("ih", SRBM_IH_BUSY),
                    ("gcatcl2", SRBM_GCATCL2_BUSY),
                    ("bif", SRBM_BIF_BUSY),
                    ("grbm_rq", SRBM_GRBM_RQ_PENDING),
                ]
            )
        ));
        lines.push(format!(
            "srbm_status2 {:#010x} busy [{}]",
            self.srbm2,
            busy_bits(
                self.srbm2,
                &[("sdma", SRBM2_SDMA_BUSY), ("sdma1", SRBM2_SDMA1_BUSY)]
            )
        ));
        if self.faults.is_empty() {
            lines.push("vm fault none".to_string());
        }
        for fault in &self.faults {
            let block: String = fault
                .mcclient
                .to_be_bytes()
                .iter()
                .map(|b| {
                    if b.is_ascii_graphic() || *b == b' ' {
                        *b as char
                    } else {
                        '?'
                    }
                })
                .collect();
            lines.push(format!(
                "vm fault ctx{} page {:#x} {} mc_client '{}' ({:#010x}) mc_id {} prot {:#04x} vmid {}",
                fault.context,
                fault.page,
                if fault.write { "write" } else { "read" },
                block,
                fault.mcclient,
                fault.memory_client_id,
                fault.protections,
                fault.vmid
            ));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
    }

    impl FakeBar {
        fn new(words: usize) -> Self {
            Self {
                cells: RefCell::new(vec![0; words]),
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    #[test]
    fn snapshot_is_empty_on_clean_device() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        let status = snapshot(&mmio);
        assert!(status.faults.is_empty());
        assert_eq!(status.grbm, 0);
        assert!(status.report().contains("vm fault none"));
    }

    #[test]
    fn fault_fields_decode_per_kernel_masks() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        let status_word =
            (0x7f) | (0x123 << FAULT_MEMORY_CLIENT_ID_SHIFT) | (1 << 24) | (5 << FAULT_VMID_SHIFT);
        mmio.bar().cells.borrow_mut()[VM_CONTEXT1_PROTECTION_FAULT_STATUS as usize] = status_word;
        mmio.bar().cells.borrow_mut()[VM_CONTEXT1_PROTECTION_FAULT_ADDR as usize] = 0xdead_beef;
        mmio.bar().cells.borrow_mut()[VM_CONTEXT1_PROTECTION_FAULT_MCCLIENT as usize] =
            u32::from_be_bytes(*b"GDS ");
        let status = snapshot(&mmio);
        assert_eq!(status.faults.len(), 1);
        let fault = &status.faults[0];
        assert_eq!(fault.context, 1);
        assert_eq!(fault.protections, 0x7f);
        assert_eq!(fault.memory_client_id, 0x123);
        assert!(fault.write);
        assert_eq!(fault.vmid, 5);
        assert_eq!(fault.page, 0xdead_beef);
        let report = status.report();
        assert!(report.contains("ctx1"));
        assert!(report.contains("write"));
        assert!(report.contains("GDS "));
    }

    #[test]
    fn clean_context_does_not_show_as_fault() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        mmio.bar().cells.borrow_mut()[VM_CONTEXT0_PROTECTION_FAULT_ADDR as usize] = 0;
        mmio.bar().cells.borrow_mut()[VM_CONTEXT0_PROTECTION_FAULT_STATUS as usize] = 0;
        assert!(snapshot(&mmio).faults.is_empty());
    }

    #[test]
    fn clear_fault_sets_cntl2_bit() {
        let mmio = Mmio::new(FakeBar::new(0x10000));
        mmio.bar().cells.borrow_mut()[VM_CONTEXT1_CNTL2 as usize] = 0x4;
        clear_fault(&mmio, 1);
        assert_eq!(mmio.bar().cells.borrow()[VM_CONTEXT1_CNTL2 as usize], 0x5);
    }

    #[test]
    fn busy_bits_lists_only_set_flags() {
        assert_eq!(
            busy_bits(0x300, &[("vmc", 0x100), ("mcb", 0x200)]),
            "vmc,mcb"
        );
        assert_eq!(busy_bits(0, &[("vmc", 0x100)]), "");
    }
}
