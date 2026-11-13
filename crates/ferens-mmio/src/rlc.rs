use std::thread;
use std::time::Duration;

use ferens_core::{BarOps, Mmio};

use crate::fw::UcodeFirmware;

pub const RLC_CNTL: u32 = 0xEC00;
pub const RLC_PG_CNTL: u32 = 0xEC43;
pub const RLC_SERDES_CU_MASTER_BUSY: u32 = 0xEC61;
pub const RLC_SERDES_NONCU_MASTER_BUSY: u32 = 0xEC62;
pub const RLC_CGCG_CGLS_CTRL: u32 = 0xEC49;
pub const RLC_CGCG_CGLS_CTRL_3D: u32 = 0xEC9D;
pub const RLC_GPM_UCODE_ADDR: u32 = 0xF83C;
pub const RLC_GPM_UCODE_DATA: u32 = 0xF83D;
pub const GRBM_SOFT_RESET: u32 = 0x2008;
pub const GRBM_GFX_INDEX: u32 = 0xC200;

pub const RLC_CNTL_RLC_ENABLE: u32 = 1;
pub const CGCG_CGLS_EN: u32 = 0x3;
pub const GRBM_SOFT_RESET_RLC: u32 = 0x4;
pub const SERDES_NONCU_BUSY_MASK: u32 = 0xD_FFFF;
pub const GRBM_GFX_INDEX_SE_INDEX_SHIFT: u32 = 16;
pub const GRBM_GFX_INDEX_SH_BROADCAST: u32 = 1 << 29;
pub const GRBM_GFX_INDEX_INSTANCE_BROADCAST: u32 = 1 << 30;
pub const GRBM_GFX_INDEX_SE_BROADCAST: u32 = 1 << 31;

pub const SHADER_ENGINES: u32 = 4;
pub const SERDES_ATTEMPTS: u32 = 100;
pub const SERDES_TICK: Duration = Duration::from_micros(1);
pub const SETTLE: Duration = Duration::from_micros(50);

pub fn serdes_idle<B: BarOps>(regs: &Mmio<B>) {
    for se in 0..SHADER_ENGINES {
        regs.write_reg_direct(
            GRBM_GFX_INDEX,
            (se << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
        );
        for _ in 0..SERDES_ATTEMPTS {
            if regs.read_reg_direct(RLC_SERDES_CU_MASTER_BUSY) == 0 {
                break;
            }
            thread::sleep(SERDES_TICK);
        }
    }
    regs.write_reg_direct(
        GRBM_GFX_INDEX,
        GRBM_GFX_INDEX_SE_BROADCAST
            | GRBM_GFX_INDEX_SH_BROADCAST
            | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
    );
    for _ in 0..SERDES_ATTEMPTS {
        if regs.read_reg_direct(RLC_SERDES_NONCU_MASTER_BUSY) & SERDES_NONCU_BUSY_MASK == 0 {
            break;
        }
        thread::sleep(SERDES_TICK);
    }
}

pub fn stop<B: BarOps>(regs: &Mmio<B>) {
    let ctrl = regs.read_reg_direct(RLC_CNTL);
    regs.write_reg_direct(RLC_CNTL, ctrl & !RLC_CNTL_RLC_ENABLE);
    serdes_idle(regs);
}

pub fn reset<B: BarOps>(regs: &Mmio<B>) {
    let soft = regs.read_reg_direct(GRBM_SOFT_RESET);
    regs.write_reg_direct(
        GRBM_SOFT_RESET,
        (soft & !GRBM_SOFT_RESET_RLC) | GRBM_SOFT_RESET_RLC,
    );
    thread::sleep(SETTLE);
    let soft = regs.read_reg_direct(GRBM_SOFT_RESET);
    regs.write_reg_direct(GRBM_SOFT_RESET, soft & !GRBM_SOFT_RESET_RLC);
    thread::sleep(SETTLE);
}

pub fn load_ucode<B: BarOps>(regs: &Mmio<B>, firmware: &UcodeFirmware) {
    regs.write_reg_direct(RLC_GPM_UCODE_ADDR, 0);
    for word in &firmware.words {
        regs.write_reg_direct(RLC_GPM_UCODE_DATA, *word);
    }
    regs.write_reg_direct(RLC_GPM_UCODE_ADDR, firmware.version);
}

pub fn enable<B: BarOps>(regs: &Mmio<B>) {
    let ctrl = regs.read_reg_direct(RLC_CNTL);
    regs.write_reg_direct(
        RLC_CNTL,
        (ctrl & !RLC_CNTL_RLC_ENABLE) | RLC_CNTL_RLC_ENABLE,
    );
    thread::sleep(SETTLE);
}

pub fn resume<B: BarOps>(regs: &Mmio<B>, firmware: &UcodeFirmware) {
    stop(regs);
    let cg = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL);
    regs.write_reg_direct(RLC_CGCG_CGLS_CTRL, cg & !CGCG_CGLS_EN);
    let cg3d = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL_3D);
    regs.write_reg_direct(RLC_CGCG_CGLS_CTRL_3D, cg3d & !CGCG_CGLS_EN);
    regs.write_reg_direct(RLC_PG_CNTL, 0);
    reset(regs);
    load_ucode(regs, firmware);
    enable(regs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
    }

    impl FakeBar {
        fn new(words: usize) -> Self {
            Self {
                cells: RefCell::new(vec![0; words]),
                writes: RefCell::new(Vec::new()),
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    const BAR_WORDS: usize = 0x10000;

    #[test]
    fn stop_disables_rlc_and_walks_shader_engines() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_CNTL as usize] = 0x3;
        stop(&mmio);
        let expected: Vec<(usize, u32)> = [
            (RLC_CNTL as usize * 4, 0x2),
            (
                GRBM_GFX_INDEX as usize * 4,
                GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                GRBM_GFX_INDEX as usize * 4,
                (1 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                GRBM_GFX_INDEX as usize * 4,
                (2 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                GRBM_GFX_INDEX as usize * 4,
                (3 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                GRBM_GFX_INDEX as usize * 4,
                GRBM_GFX_INDEX_SE_BROADCAST
                    | GRBM_GFX_INDEX_SH_BROADCAST
                    | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
        ]
        .to_vec();
        assert_eq!(mmio.bar().writes.borrow().as_slice(), expected.as_slice());
    }

    #[test]
    fn reset_toggles_soft_reset_rlc_bit() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[GRBM_SOFT_RESET as usize] = 0x0001_0100;
        reset(&mmio);
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[
                (GRBM_SOFT_RESET as usize * 4, 0x0001_0104),
                (GRBM_SOFT_RESET as usize * 4, 0x0001_0100)
            ]
        );
    }

    #[test]
    fn ucode_load_streams_dwords_and_stamps_version() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        let firmware = UcodeFirmware {
            version: 0x11E,
            feature_version: 1,
            words: vec![0xBEEF_0001, 0xBEEF_0002],
        };
        load_ucode(&mmio, &firmware);
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[
                (RLC_GPM_UCODE_ADDR as usize * 4, 0),
                (RLC_GPM_UCODE_DATA as usize * 4, 0xBEEF_0001),
                (RLC_GPM_UCODE_DATA as usize * 4, 0xBEEF_0002),
                (RLC_GPM_UCODE_ADDR as usize * 4, 0x11E)
            ]
        );
    }

    #[test]
    fn enable_sets_rlc_enable_bit_only() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_CNTL as usize] = 0x10;
        enable(&mmio);
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[(RLC_CNTL as usize * 4, 0x11)]
        );
    }

    #[test]
    fn resume_follows_kernel_rlc_resume_order() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        let firmware = UcodeFirmware {
            version: 0x11E,
            feature_version: 1,
            words: vec![0xBEEF_0001, 0xBEEF_0002],
        };
        resume(&mmio, &firmware);
        let ggi = GRBM_GFX_INDEX as usize * 4;
        let expected: Vec<(usize, u32)> = [
            (RLC_CNTL as usize * 4, 0),
            (ggi, GRBM_GFX_INDEX_INSTANCE_BROADCAST),
            (
                ggi,
                (1 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                ggi,
                (2 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                ggi,
                (3 << GRBM_GFX_INDEX_SE_INDEX_SHIFT) | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (
                ggi,
                GRBM_GFX_INDEX_SE_BROADCAST
                    | GRBM_GFX_INDEX_SH_BROADCAST
                    | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
            ),
            (RLC_CGCG_CGLS_CTRL as usize * 4, 0),
            (RLC_CGCG_CGLS_CTRL_3D as usize * 4, 0),
            (RLC_PG_CNTL as usize * 4, 0),
            (GRBM_SOFT_RESET as usize * 4, GRBM_SOFT_RESET_RLC),
            (GRBM_SOFT_RESET as usize * 4, 0),
            (RLC_GPM_UCODE_ADDR as usize * 4, 0),
            (RLC_GPM_UCODE_DATA as usize * 4, 0xBEEF_0001),
            (RLC_GPM_UCODE_DATA as usize * 4, 0xBEEF_0002),
            (RLC_GPM_UCODE_ADDR as usize * 4, 0x11E),
            (RLC_CNTL as usize * 4, RLC_CNTL_RLC_ENABLE),
        ]
        .to_vec();
        assert_eq!(mmio.bar().writes.borrow().as_slice(), expected.as_slice());
    }

    #[test]
    fn serdes_poll_is_bounded_and_nonfatal() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_SERDES_CU_MASTER_BUSY as usize] = 0xF00D;
        mmio.bar().cells.borrow_mut()[RLC_SERDES_NONCU_MASTER_BUSY as usize] =
            SERDES_NONCU_BUSY_MASK;
        stop(&mmio);
        assert_eq!(mmio.bar().cells.borrow()[RLC_CNTL as usize], 0);
        let ggi = mmio
            .bar()
            .writes
            .borrow()
            .iter()
            .find(|entry| entry.0 == GRBM_GFX_INDEX as usize * 4)
            .copied();
        assert_eq!(
            ggi,
            Some((
                GRBM_GFX_INDEX as usize * 4,
                GRBM_GFX_INDEX_INSTANCE_BROADCAST
            ))
        );
    }
}
