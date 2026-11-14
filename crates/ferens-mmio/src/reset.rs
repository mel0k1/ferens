use std::io;
use std::thread;
use std::time::Duration;

use ferens_core::{BarOps, Mmio};

// Регистры soft reset — dword-индексы из gfx_8_0_d.h, oss_3_0_d.h и gmc_8_1_d.h.
pub const GRBM_STATUS: u32 = 0x2004;
pub const GRBM_STATUS2: u32 = 0x2002;
pub const GRBM_SOFT_RESET: u32 = 0x2008;
pub const SRBM_STATUS: u32 = 0x394;
pub const SRBM_STATUS2: u32 = 0x393;
pub const SRBM_SOFT_RESET: u32 = 0x398;
pub const GMCON_DEBUG: u32 = 0xd5f;

// Busy-биты GRBM_STATUS (gfx_8_0_sh_mask.h) — та же дизъюнкция, что в
// gfx_v8_0_check_soft_reset: PA/SC/BCI/SX/TA/VGT/DB/CB/GDS/SPI/IA/IA_NO_DMA/CP/CP_COHERENCY.
pub const GFX_BUSY: u32 = 0x4000
    | 0x8000
    | 0x20000
    | 0x40000
    | 0x80000
    | 0x100000
    | 0x400000
    | 0x800000
    | 0x1000000
    | 0x2000000
    | 0x4000000
    | 0x10000000
    | 0x20000000
    | 0x40000000;
pub const GRBM_STATUS2_RLC_BUSY: u32 = 0x1000000;
pub const GRBM_STATUS2_CPX_BUSY: u32 = 0x10000000 | 0x20000000 | 0x40000000;

// Busy-биты SRBM_STATUS/SRBM_STATUS2 (oss_3_0_sh_mask.h) — чеки gmc_v8_0_check_soft_reset
// и sdma_v3_0_check_soft_reset.
pub const SRBM_STATUS_GRBM_RQ_PENDING: u32 = 0x20;
pub const SRBM_STATUS_VMC_BUSY: u32 = 0x100;
pub const SRBM_STATUS_MC_BUSY: u32 = 0x200 | 0x400 | 0x800 | 0x1000;
pub const SRBM_STATUS_SEM_BUSY: u32 = 0x4000;
pub const SRBM_STATUS2_SDMA_BUSY: u32 = 0x20;
pub const SRBM_STATUS2_SDMA1_BUSY: u32 = 0x40;

// Биты GRBM_SOFT_RESET (gfx_8_0_sh_mask.h).
pub const GRBM_SOFT_RESET_CP: u32 = 0x1;
pub const GRBM_SOFT_RESET_RLC: u32 = 0x4;
pub const GRBM_SOFT_RESET_GFX: u32 = 0x1_0000;
pub const GRBM_SOFT_RESET_CPF: u32 = 0x2_0000;
pub const GRBM_SOFT_RESET_CPC: u32 = 0x4_0000;
pub const GRBM_SOFT_RESET_CPG: u32 = 0x8_0000;

// Биты SRBM_SOFT_RESET (oss_3_0_sh_mask.h).
pub const SRBM_SOFT_RESET_SDMA1: u32 = 0x40;
pub const SRBM_SOFT_RESET_GRBM: u32 = 0x100;
pub const SRBM_SOFT_RESET_MC: u32 = 0x800;
pub const SRBM_SOFT_RESET_SEM: u32 = 0x8000;
pub const SRBM_SOFT_RESET_VMC: u32 = 0x2_0000;
pub const SRBM_SOFT_RESET_SDMA: u32 = 0x10_0000;

// GMCON_DEBUG: GFX_STALL и GFX_CLEAR (gmc_8_1_sh_mask.h).
pub const GMCON_DEBUG_GFX_STALL: u32 = 0x1;
pub const GMCON_DEBUG_GFX_CLEAR: u32 = 0x2;

// udelay(50) из gfx_v8_0_soft_reset.
pub const RESET_SETTLE: Duration = Duration::from_micros(50);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoftResetPlan {
    pub grbm: u32,
    pub srbm: u32,
}

impl SoftResetPlan {
    pub fn is_empty(self) -> bool {
        self.grbm == 0 && self.srbm == 0
    }

    pub fn report(self) -> String {
        let mut grbm = Vec::new();
        if self.grbm & GRBM_SOFT_RESET_CP != 0 {
            grbm.push("CP");
        }
        if self.grbm & GRBM_SOFT_RESET_RLC != 0 {
            grbm.push("RLC");
        }
        if self.grbm & GRBM_SOFT_RESET_GFX != 0 {
            grbm.push("GFX");
        }
        if self.grbm & GRBM_SOFT_RESET_CPF != 0 {
            grbm.push("CPF");
        }
        if self.grbm & GRBM_SOFT_RESET_CPC != 0 {
            grbm.push("CPC");
        }
        if self.grbm & GRBM_SOFT_RESET_CPG != 0 {
            grbm.push("CPG");
        }
        let mut srbm = Vec::new();
        if self.srbm & SRBM_SOFT_RESET_GRBM != 0 {
            srbm.push("GRBM");
        }
        if self.srbm & SRBM_SOFT_RESET_SDMA != 0 {
            srbm.push("SDMA");
        }
        if self.srbm & SRBM_SOFT_RESET_SDMA1 != 0 {
            srbm.push("SDMA1");
        }
        if self.srbm & SRBM_SOFT_RESET_VMC != 0 {
            srbm.push("VMC");
        }
        if self.srbm & SRBM_SOFT_RESET_MC != 0 {
            srbm.push("MC");
        }
        if self.srbm & SRBM_SOFT_RESET_SEM != 0 {
            srbm.push("SEM");
        }
        format!(
            "GRBM_SOFT_RESET=0x{:08X} ({}), SRBM_SOFT_RESET=0x{:08X} ({})",
            self.grbm,
            grbm.join(" "),
            self.srbm,
            srbm.join(" ")
        )
    }
}

// gfx_v8_0_check_soft_reset + gmc_v8_0_check_soft_reset + busy-чек SDMA:
// маски собираются по занятым блокам, как adev->gfx.grbm_soft_reset/srbm_soft_reset.
pub fn plan_soft_reset<B: BarOps>(regs: &Mmio<B>) -> SoftResetPlan {
    let mut plan = SoftResetPlan::default();
    let grbm_status = regs.read_reg_direct(GRBM_STATUS);
    if grbm_status & GFX_BUSY != 0 {
        plan.grbm |= GRBM_SOFT_RESET_CP | GRBM_SOFT_RESET_GFX;
        plan.srbm |= SRBM_SOFT_RESET_GRBM;
    }
    let grbm_status2 = regs.read_reg_direct(GRBM_STATUS2);
    if grbm_status2 & GRBM_STATUS2_RLC_BUSY != 0 {
        plan.grbm |= GRBM_SOFT_RESET_RLC;
    }
    if grbm_status2 & GRBM_STATUS2_CPX_BUSY != 0 {
        plan.grbm |= GRBM_SOFT_RESET_CPF | GRBM_SOFT_RESET_CPC | GRBM_SOFT_RESET_CPG;
        plan.srbm |= SRBM_SOFT_RESET_GRBM;
    }
    let srbm_status = regs.read_reg_direct(SRBM_STATUS);
    if srbm_status & SRBM_STATUS_GRBM_RQ_PENDING != 0 {
        plan.srbm |= SRBM_SOFT_RESET_GRBM;
    }
    if srbm_status & SRBM_STATUS_SEM_BUSY != 0 {
        plan.srbm |= SRBM_SOFT_RESET_SEM;
    }
    if srbm_status & SRBM_STATUS_VMC_BUSY != 0 {
        plan.srbm |= SRBM_SOFT_RESET_VMC;
    }
    // gmc ставит SOFT_RESET_MC только на dGPU — ferens работает именно с ними.
    if srbm_status & SRBM_STATUS_MC_BUSY != 0 {
        plan.srbm |= SRBM_SOFT_RESET_MC;
    }
    let srbm_status2 = regs.read_reg_direct(SRBM_STATUS2);
    if srbm_status2 & SRBM_STATUS2_SDMA_BUSY != 0 {
        plan.srbm |= SRBM_SOFT_RESET_SDMA;
    }
    if srbm_status2 & SRBM_STATUS2_SDMA1_BUSY != 0 {
        plan.srbm |= SRBM_SOFT_RESET_SDMA1;
    }
    plan
}

// gfx_v8_0_soft_reset: GMCON GFX_STALL|GFX_CLEAR, установка и снятие битов
// GRBM_SOFT_RESET и SRBM_SOFT_RESET с паузами 50 мкс и readback-ами.
pub fn apply_soft_reset<B: BarOps>(regs: &Mmio<B>, plan: SoftResetPlan) -> io::Result<()> {
    if plan.is_empty() {
        return Ok(());
    }
    let gmcon = regs.read_reg_direct(GMCON_DEBUG);
    regs.write_reg_direct(
        GMCON_DEBUG,
        gmcon | GMCON_DEBUG_GFX_STALL | GMCON_DEBUG_GFX_CLEAR,
    );
    thread::sleep(RESET_SETTLE);

    let grbm = regs.read_reg_direct(GRBM_SOFT_RESET) | plan.grbm;
    regs.write_reg_direct(GRBM_SOFT_RESET, grbm);
    let _ = regs.read_reg_direct(GRBM_SOFT_RESET);
    thread::sleep(RESET_SETTLE);
    regs.write_reg_direct(GRBM_SOFT_RESET, grbm & !plan.grbm);
    let _ = regs.read_reg_direct(GRBM_SOFT_RESET);

    let srbm = regs.read_reg_direct(SRBM_SOFT_RESET) | plan.srbm;
    regs.write_reg_direct(SRBM_SOFT_RESET, srbm);
    let _ = regs.read_reg_direct(SRBM_SOFT_RESET);
    thread::sleep(RESET_SETTLE);
    regs.write_reg_direct(SRBM_SOFT_RESET, srbm & !plan.srbm);
    let _ = regs.read_reg_direct(SRBM_SOFT_RESET);

    let gmcon = regs.read_reg_direct(GMCON_DEBUG);
    regs.write_reg_direct(
        GMCON_DEBUG,
        gmcon & !(GMCON_DEBUG_GFX_STALL | GMCON_DEBUG_GFX_CLEAR),
    );
    thread::sleep(RESET_SETTLE);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct ResetBar {
        cells: RefCell<HashMap<usize, u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
    }

    impl ResetBar {
        fn new() -> Self {
            Self {
                cells: RefCell::new(HashMap::new()),
                writes: RefCell::new(Vec::new()),
            }
        }

        fn set(&self, index: u32, value: u32) {
            // BarOps работает байтовыми смещениями — dword-индекс * 4
            self.cells.borrow_mut().insert(index as usize * 4, value);
        }

        fn write_record(&self) -> Vec<(u32, u32)> {
            self.writes
                .borrow()
                .iter()
                .map(|(offset, value)| ((offset / 4) as u32, *value))
                .collect()
        }
    }

    impl BarOps for ResetBar {
        fn read_u32(&self, offset: usize) -> u32 {
            *self.cells.borrow().get(&offset).unwrap_or(&0)
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
            self.cells.borrow_mut().insert(offset, value);
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    #[test]
    fn plan_collects_masks_from_busy_blocks() {
        let bar = ResetBar::new();
        bar.set(GRBM_STATUS, 0x4000_0000); // CB_BUSY
        bar.set(GRBM_STATUS2, GRBM_STATUS2_RLC_BUSY | GRBM_STATUS2_CPX_BUSY);
        bar.set(SRBM_STATUS, SRBM_STATUS_VMC_BUSY | SRBM_STATUS_SEM_BUSY);
        bar.set(SRBM_STATUS2, SRBM_STATUS2_SDMA1_BUSY);
        let mmio = Mmio::new(bar);
        let plan = plan_soft_reset(&mmio);
        assert_eq!(
            plan.grbm,
            GRBM_SOFT_RESET_CP
                | GRBM_SOFT_RESET_GFX
                | GRBM_SOFT_RESET_RLC
                | GRBM_SOFT_RESET_CPF
                | GRBM_SOFT_RESET_CPC
                | GRBM_SOFT_RESET_CPG
        );
        assert_eq!(
            plan.srbm,
            SRBM_SOFT_RESET_GRBM
                | SRBM_SOFT_RESET_VMC
                | SRBM_SOFT_RESET_SEM
                | SRBM_SOFT_RESET_SDMA1
        );
        assert!(!plan.is_empty());
        let report = plan.report();
        assert!(report.contains("GRBM_SOFT_RESET=0x000F0005"));
        assert!(report.contains("CP RLC GFX CPF CPC CPG"));
        assert!(report.contains("SRBM_SOFT_RESET=0x00028140"));
        assert!(report.contains("GRBM SDMA1 VMC SEM"));
    }

    #[test]
    fn idle_card_produces_empty_plan() {
        let bar = ResetBar::new();
        let mmio = Mmio::new(bar);
        let plan = plan_soft_reset(&mmio);
        assert!(plan.is_empty());
        assert_eq!(
            plan.report(),
            "GRBM_SOFT_RESET=0x00000000 (), SRBM_SOFT_RESET=0x00000000 ()"
        );
    }

    #[test]
    fn apply_stalls_gmcon_and_resets_only_planned_bits() {
        let bar = ResetBar::new();
        bar.set(GRBM_SOFT_RESET, 0x2); // посторонний бит, должен выжить
        bar.set(SRBM_SOFT_RESET, 0x4);
        bar.set(GMCON_DEBUG, 0x10);
        let mmio = Mmio::new(bar);
        let plan = SoftResetPlan {
            grbm: GRBM_SOFT_RESET_CP | GRBM_SOFT_RESET_RLC,
            srbm: SRBM_SOFT_RESET_VMC,
        };
        apply_soft_reset(&mmio, plan).unwrap();
        let writes = mmio.bar().write_record();
        let gmcon_writes: Vec<u32> = writes
            .iter()
            .filter(|(index, _)| *index == GMCON_DEBUG)
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(gmcon_writes, vec![0x10 | 0x3, 0x10]);
        let grbm_writes: Vec<u32> = writes
            .iter()
            .filter(|(index, _)| *index == GRBM_SOFT_RESET)
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(grbm_writes, vec![0x2 | 0x5, 0x2]);
        let srbm_writes: Vec<u32> = writes
            .iter()
            .filter(|(index, _)| *index == SRBM_SOFT_RESET)
            .map(|(_, value)| *value)
            .collect();
        assert_eq!(srbm_writes, vec![0x4 | SRBM_SOFT_RESET_VMC, 0x4]);
        assert_eq!(mmio.read_reg_direct(GRBM_SOFT_RESET), 0x2);
    }

    #[test]
    fn empty_plan_skips_all_writes() {
        let bar = ResetBar::new();
        let mmio = Mmio::new(bar);
        apply_soft_reset(&mmio, SoftResetPlan::default()).unwrap();
        assert!(mmio.bar().write_record().is_empty());
    }
}
