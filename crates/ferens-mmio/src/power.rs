use std::io;

use ferens_core::{BarOps, Mmio};

use crate::fw::RlcRegisterList;
use crate::rlc::{
    serdes_idle, GRBM_GFX_INDEX, GRBM_GFX_INDEX_INSTANCE_BROADCAST, GRBM_GFX_INDEX_SE_BROADCAST,
    GRBM_GFX_INDEX_SH_BROADCAST, RLC_CGCG_CGLS_CTRL, RLC_CGCG_CGLS_CTRL_3D, RLC_PG_CNTL,
};

pub const RLC_CGTT_MGCG_OVERRIDE: u32 = 0xEC48;
pub const RLC_SERDES_WR_CU_MASTER_MASK: u32 = 0xEC5D;
pub const RLC_SERDES_WR_NONCU_MASTER_MASK: u32 = 0xEC5E;
pub const RLC_SERDES_WR_CTRL: u32 = 0xEC5F;
pub const RLC_GPM_SCRATCH_ADDR: u32 = 0xEC6C;
pub const RLC_GPM_SCRATCH_DATA: u32 = 0xEC6D;
pub const RLC_SRM_CNTL: u32 = 0xEC80;
pub const RLC_SRM_ARAM_ADDR: u32 = 0xEC83;
pub const RLC_SRM_ARAM_DATA: u32 = 0xEC84;
pub const RLC_SRM_INDEX_CNTL_ADDR_0: u32 = 0xEC8B;
pub const RLC_SRM_INDEX_CNTL_DATA_0: u32 = 0xEC93;
pub const RLC_PG_DELAY: u32 = 0xEC4D;
pub const RLC_PG_DELAY_2: u32 = 0xEC1F;
pub const RLC_AUTO_PG_CTRL: u32 = 0xEC55;
pub const RLC_CSIB_ADDR_LO: u32 = 0xECA2;
pub const RLC_CSIB_ADDR_HI: u32 = 0xECA3;
pub const RLC_CSIB_LENGTH: u32 = 0xECA4;
pub const CP_RB_WPTR_POLL_CNTL: u32 = 0x21C2;
pub const CP_INT_CNTL_RING0: u32 = 0x306A;
pub const CB_CGTT_SCLK_CTRL: u32 = 0xF0A8;

pub const CGCG_EN: u32 = 1;
pub const CGLS_EN: u32 = 2;
pub const CGCG_CGLS_EN_MASK: u32 = 0x3;
pub const CGLS_REP_COMPANSAT_DELAY: u32 = 0xF << 2;
pub const CGCG_GFX_IDLE_THRESHOLD: u32 = 0x100 << 8;
pub const OVERRIDE_CGCG: u32 = 1 << 3;
pub const OVERRIDE_CGLS: u32 = 1 << 4;

pub const PG_CNTL_DYN_PER_CU_PG: u32 = 1 << 2;
pub const PG_CNTL_STATIC_PER_CU_PG: u32 = 1 << 3;

pub const SRM_AUTO_INCR_ADDR: u32 = 1 << 1;
pub const SRM_ENABLE: u32 = 1;
pub const SRM_INDEX_RANGE_MASK: u32 = 0x0003_FFFF;

pub const SERDES_WR_POWER_DOWN: u32 = 1 << 8;
pub const SERDES_WR_POWER_UP: u32 = 1 << 9;
pub const SERDES_WR_P1_SELECT: u32 = 1 << 10;
pub const SERDES_WR_P2_SELECT: u32 = 1 << 11;
pub const SERDES_WR_WRITE_COMMAND: u32 = 1 << 12;
pub const SERDES_WR_READ_COMMAND: u32 = 1 << 13;
pub const SERDES_WR_RDDATA_RESET: u32 = 1 << 14;
pub const SERDES_WR_SHORT_FORMAT: u32 = 1 << 15;
pub const SERDES_WR_BPM_DATA_SHIFT: u32 = 16;
pub const SERDES_WR_BPM_DATA_MASK: u32 = 0x3FF << 16;
pub const SERDES_WR_SRBM_OVERRIDE: u32 = 1 << 26;
pub const SERDES_WR_RSVD_BPM_ADDR: u32 = 1 << 27;
pub const SERDES_WR_REG_ADDR_SHIFT: u32 = 28;
pub const SERDES_WR_REG_ADDR_MASK: u32 = 0xF << 28;
pub const SERDES_WR_BPM_ADDR: u32 = 0xFF;

pub const PG_DELAY_ALL: u32 = 0x1010_1010;
pub const PG_DELAY_2_SERDES_CMD_DELAY: u32 = 3 << 8;
pub const PG_DELAY_2_SERDES_CMD_DELAY_MASK: u32 = 0xFF << 8;
pub const AUTO_PG_CTRL_IDLE_THRESHOLD: u32 = 0x55F0 << 3;
pub const AUTO_PG_CTRL_IDLE_THRESHOLD_MASK: u32 = 0x0007_FFF8;
pub const WPTR_POLL_IDLE_COUNT: u32 = 0x60 << 16;
pub const WPTR_POLL_IDLE_COUNT_MASK: u32 = 0xFFFF << 16;

pub const GUI_IDLE_INTERRUPTS: u32 = (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21);
pub const CG_WAKE_READS: u32 = 4;
pub const FORMAT_DIRECT_REG_LIST_LENGTH: usize = 14;

pub const BPM_REG_CGLS_EN: u32 = 0;
pub const BPM_REG_CGCG_OVERRIDE: u32 = 2;
pub const BPM_REG_MGCG_OVERRIDE: u32 = 3;
pub const BPM_CMD_CLEAR: u32 = 0;
pub const BPM_CMD_SET: u32 = 1;

pub struct IndirectList {
    pub starts: [usize; 10],
    pub start_count: usize,
    pub unique: [u32; 8],
    pub unique_count: usize,
}

pub struct PgSetup<'a> {
    pub csb: &'a [u32],
    pub csb_va: u64,
    pub list: &'a RlcRegisterList,
}

pub fn serdes_command<B: BarOps>(regs: &Mmio<B>, bpm_reg: u32, cmd: u32) {
    regs.write_reg_direct(
        GRBM_GFX_INDEX,
        GRBM_GFX_INDEX_SE_BROADCAST
            | GRBM_GFX_INDEX_SH_BROADCAST
            | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
    );
    regs.write_reg_direct(RLC_SERDES_WR_CU_MASTER_MASK, 0xFFFF_FFFF);
    regs.write_reg_direct(RLC_SERDES_WR_NONCU_MASTER_MASK, 0xFFFF_FFFF);
    let ctrl = regs.read_reg_direct(RLC_SERDES_WR_CTRL);
    let ctrl = ctrl
        & !(SERDES_WR_WRITE_COMMAND
            | SERDES_WR_READ_COMMAND
            | SERDES_WR_P1_SELECT
            | SERDES_WR_P2_SELECT
            | SERDES_WR_RDDATA_RESET
            | SERDES_WR_POWER_DOWN
            | SERDES_WR_POWER_UP
            | SERDES_WR_SHORT_FORMAT
            | SERDES_WR_BPM_DATA_MASK
            | SERDES_WR_REG_ADDR_MASK
            | SERDES_WR_SRBM_OVERRIDE);
    let ctrl = ctrl
        | SERDES_WR_RSVD_BPM_ADDR
        | (cmd << SERDES_WR_BPM_DATA_SHIFT)
        | (bpm_reg << SERDES_WR_REG_ADDR_SHIFT)
        | SERDES_WR_BPM_ADDR;
    regs.write_reg_direct(RLC_SERDES_WR_CTRL, ctrl);
}

pub fn gui_idle<B: BarOps>(regs: &Mmio<B>, enable: bool) {
    let value = regs.read_reg_direct(CP_INT_CNTL_RING0);
    let value = if enable {
        value | GUI_IDLE_INTERRUPTS
    } else {
        value & !GUI_IDLE_INTERRUPTS
    };
    regs.write_reg_direct(CP_INT_CNTL_RING0, value);
}

pub fn cg_on<B: BarOps>(regs: &Mmio<B>) {
    let data = regs.read_reg_direct(RLC_CGTT_MGCG_OVERRIDE);
    regs.write_reg_direct(
        RLC_CGTT_MGCG_OVERRIDE,
        data & !(OVERRIDE_CGCG | OVERRIDE_CGLS),
    );
    serdes_idle(regs);
    serdes_command(regs, BPM_REG_CGCG_OVERRIDE, BPM_CMD_CLEAR);
    serdes_idle(regs);
    serdes_command(regs, BPM_REG_CGLS_EN, BPM_CMD_SET);
    let data = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL);
    let data = (data & !CGCG_CGLS_EN_MASK)
        | CGCG_EN
        | CGLS_EN
        | CGLS_REP_COMPANSAT_DELAY
        | CGCG_GFX_IDLE_THRESHOLD;
    regs.write_reg_direct(RLC_CGCG_CGLS_CTRL, data);
    let data3d = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL_3D);
    regs.write_reg_direct(
        RLC_CGCG_CGLS_CTRL_3D,
        (data3d & !CGCG_CGLS_EN_MASK) | CGLS_REP_COMPANSAT_DELAY | CGCG_GFX_IDLE_THRESHOLD,
    );
    gui_idle(regs, true);
    serdes_idle(regs);
}

pub fn cg_off<B: BarOps>(regs: &Mmio<B>) {
    gui_idle(regs, false);
    let data = regs.read_reg_direct(RLC_CGTT_MGCG_OVERRIDE);
    regs.write_reg_direct(RLC_CGTT_MGCG_OVERRIDE, data | OVERRIDE_CGCG | OVERRIDE_CGLS);
    for _ in 0..CG_WAKE_READS {
        let _ = regs.read_reg_direct(CB_CGTT_SCLK_CTRL);
    }
    serdes_idle(regs);
    serdes_command(regs, BPM_REG_CGCG_OVERRIDE, BPM_CMD_SET);
    serdes_idle(regs);
    serdes_command(regs, BPM_REG_CGLS_EN, BPM_CMD_CLEAR);
    let data = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL);
    regs.write_reg_direct(RLC_CGCG_CGLS_CTRL, data & !CGCG_CGLS_EN_MASK);
    let data3d = regs.read_reg_direct(RLC_CGCG_CGLS_CTRL_3D);
    regs.write_reg_direct(RLC_CGCG_CGLS_CTRL_3D, data3d & !CGCG_CGLS_EN_MASK);
    gui_idle(regs, true);
    serdes_idle(regs);
}

pub fn init_csb<B: BarOps>(regs: &Mmio<B>, csb: &[u32], csb_va: u64) {
    regs.write_reg_direct(RLC_CSIB_ADDR_HI, (csb_va >> 32) as u32);
    regs.write_reg_direct(RLC_CSIB_ADDR_LO, (csb_va & !0x3) as u32);
    regs.write_reg_direct(RLC_CSIB_LENGTH, csb.len() as u32);
}

pub fn parse_indirect_list(format: &mut [u32]) -> io::Result<IndirectList> {
    let mut list = IndirectList {
        starts: [0; 10],
        start_count: 0,
        unique: [0; 8],
        unique_count: 0,
    };
    let mut new_entry = true;
    let mut offset = FORMAT_DIRECT_REG_LIST_LENGTH;
    while offset < format.len() {
        if new_entry {
            new_entry = false;
            if list.start_count >= list.starts.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "rlc indirect list has too many entries",
                ));
            }
            list.starts[list.start_count] = offset;
            list.start_count += 1;
        }
        if format[offset] == 0xFFFF_FFFF {
            new_entry = true;
            offset += 1;
            continue;
        }
        offset += 2;
        if offset >= format.len() {
            break;
        }
        let mut found = list.unique_count;
        for (index, &seen) in list.unique[..list.unique_count].iter().enumerate() {
            if seen == format[offset] {
                found = index;
                break;
            }
        }
        if found == list.unique_count {
            if list.unique_count >= list.unique.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "rlc indirect list has too many unique indices",
                ));
            }
            list.unique[list.unique_count] = format[offset];
            list.unique_count += 1;
        }
        format[offset] = found as u32;
        offset += 1;
    }
    Ok(list)
}

pub fn init_save_restore_list<B: BarOps>(regs: &Mmio<B>, list: &RlcRegisterList) -> io::Result<()> {
    let mut format = list.format.clone();
    let parsed = parse_indirect_list(&mut format)?;
    let cntl = regs.read_reg_direct(RLC_SRM_CNTL);
    regs.write_reg_direct(RLC_SRM_CNTL, cntl | SRM_AUTO_INCR_ADDR);
    regs.write_reg_direct(RLC_SRM_ARAM_ADDR, 0);
    for &word in &list.restore {
        regs.write_reg_direct(RLC_SRM_ARAM_DATA, word);
    }
    regs.write_reg_direct(RLC_GPM_SCRATCH_ADDR, list.format_start);
    for &word in &format {
        regs.write_reg_direct(RLC_GPM_SCRATCH_DATA, word);
    }
    let restore_dwords = list.restore.len() as u32;
    regs.write_reg_direct(RLC_GPM_SCRATCH_ADDR, list.restore_list_size);
    regs.write_reg_direct(RLC_GPM_SCRATCH_DATA, restore_dwords >> 1);
    regs.write_reg_direct(RLC_GPM_SCRATCH_ADDR, list.offsets_start);
    for &start in parsed.starts.iter() {
        regs.write_reg_direct(RLC_GPM_SCRATCH_DATA, start as u32);
    }
    for (index, &value) in parsed.unique.iter().enumerate() {
        if value != 0 {
            regs.write_reg_direct(
                RLC_SRM_INDEX_CNTL_ADDR_0 + index as u32,
                value & SRM_INDEX_RANGE_MASK,
            );
            regs.write_reg_direct(RLC_SRM_INDEX_CNTL_DATA_0 + index as u32, value >> 20);
        }
    }
    Ok(())
}

pub fn enable_save_restore_machine<B: BarOps>(regs: &Mmio<B>) {
    let cntl = regs.read_reg_direct(RLC_SRM_CNTL);
    regs.write_reg_direct(RLC_SRM_CNTL, cntl | SRM_ENABLE);
}

pub fn init_power_gating<B: BarOps>(regs: &Mmio<B>) {
    let poll = regs.read_reg_direct(CP_RB_WPTR_POLL_CNTL);
    regs.write_reg_direct(
        CP_RB_WPTR_POLL_CNTL,
        (poll & !WPTR_POLL_IDLE_COUNT_MASK) | WPTR_POLL_IDLE_COUNT,
    );
    regs.write_reg_direct(RLC_PG_DELAY, PG_DELAY_ALL);
    let delay = regs.read_reg_direct(RLC_PG_DELAY_2);
    regs.write_reg_direct(
        RLC_PG_DELAY_2,
        (delay & !PG_DELAY_2_SERDES_CMD_DELAY_MASK) | PG_DELAY_2_SERDES_CMD_DELAY,
    );
    let auto_ctrl = regs.read_reg_direct(RLC_AUTO_PG_CTRL);
    regs.write_reg_direct(
        RLC_AUTO_PG_CTRL,
        (auto_ctrl & !AUTO_PG_CTRL_IDLE_THRESHOLD_MASK) | AUTO_PG_CTRL_IDLE_THRESHOLD,
    );
}

pub fn init_pg<B: BarOps>(regs: &Mmio<B>, setup: &PgSetup) -> io::Result<()> {
    init_csb(regs, setup.csb, setup.csb_va);
    init_save_restore_list(regs, setup.list)?;
    enable_save_restore_machine(regs);
    init_power_gating(regs);
    Ok(())
}

pub fn enable_pg<B: BarOps>(regs: &Mmio<B>) {
    let value = regs.read_reg_direct(RLC_PG_CNTL);
    regs.write_reg_direct(
        RLC_PG_CNTL,
        value | PG_CNTL_STATIC_PER_CU_PG | PG_CNTL_DYN_PER_CU_PG,
    );
}

pub fn disable_pg<B: BarOps>(regs: &Mmio<B>) {
    let value = regs.read_reg_direct(RLC_PG_CNTL);
    regs.write_reg_direct(
        RLC_PG_CNTL,
        value & !(PG_CNTL_STATIC_PER_CU_PG | PG_CNTL_DYN_PER_CU_PG),
    );
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

    const BAR_WORDS: usize = 0x20000;

    fn broadcast_index() -> (usize, u32) {
        (
            GRBM_GFX_INDEX as usize * 4,
            GRBM_GFX_INDEX_SE_BROADCAST
                | GRBM_GFX_INDEX_SH_BROADCAST
                | GRBM_GFX_INDEX_INSTANCE_BROADCAST,
        )
    }

    fn serdes_mask_writes() -> Vec<(usize, u32)> {
        vec![
            broadcast_index(),
            (RLC_SERDES_WR_CU_MASTER_MASK as usize * 4, 0xFFFF_FFFF),
            (RLC_SERDES_WR_NONCU_MASTER_MASK as usize * 4, 0xFFFF_FFFF),
        ]
    }

    #[test]
    fn serdes_command_encodes_bpm_register_and_cmd() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_SERDES_WR_CTRL as usize] = 0xFFFF_FFFF;
        serdes_command(&mmio, BPM_REG_CGCG_OVERRIDE, BPM_CMD_CLEAR);
        let expected: Vec<(usize, u32)> = serdes_mask_writes()
            .into_iter()
            .chain(std::iter::once((
                RLC_SERDES_WR_CTRL as usize * 4,
                0x2800_00FF,
            )))
            .collect();
        assert_eq!(mmio.bar().writes.borrow().as_slice(), expected.as_slice());
    }

    #[test]
    fn serdes_command_for_cgls_set_uses_bpm_reg_zero() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        serdes_command(&mmio, BPM_REG_CGLS_EN, BPM_CMD_SET);
        let writes = mmio.bar().writes.borrow();
        let ctrl = writes.last().copied().unwrap();
        assert_eq!(ctrl.1, 0x0801_00FF);
    }

    #[test]
    fn cg_on_clears_overrides_and_sets_ctrl_fields() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_CGTT_MGCG_OVERRIDE as usize] = 0x18;
        mmio.bar().cells.borrow_mut()[RLC_CGCG_CGLS_CTRL as usize] = 0x2;
        cg_on(&mmio);
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (RLC_CGTT_MGCG_OVERRIDE as usize * 4, 0));
        let ctrl = writes
            .iter()
            .find(|entry| entry.0 == RLC_CGCG_CGLS_CTRL as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(ctrl.1, 0x1_003F);
        assert_eq!(ctrl.1 & CGCG_CGLS_EN_MASK, CGCG_EN | CGLS_EN);
        let int = writes
            .iter()
            .find(|entry| entry.0 == CP_INT_CNTL_RING0 as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(int.1, GUI_IDLE_INTERRUPTS);
        let serdes_cmds: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SERDES_WR_CTRL as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(serdes_cmds, vec![0x2800_00FF, 0x0801_00FF]);
    }

    #[test]
    fn cg_off_sets_overrides_and_clears_enable_bits() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_CGCG_CGLS_CTRL as usize] = 0x1_003F;
        cg_off(&mmio);
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (CP_INT_CNTL_RING0 as usize * 4, 0));
        let override_write = writes
            .iter()
            .find(|entry| entry.0 == RLC_CGTT_MGCG_OVERRIDE as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(override_write.1, OVERRIDE_CGCG | OVERRIDE_CGLS);
        let ctrl = writes
            .iter()
            .find(|entry| entry.0 == RLC_CGCG_CGLS_CTRL as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(ctrl.1, 0x1_003C);
        let serdes_cmds: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SERDES_WR_CTRL as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(serdes_cmds, vec![0x2801_00FF, 0x0800_00FF]);
    }

    #[test]
    fn indirect_list_dedupes_indices_and_records_starts() {
        let mut format = vec![0u32; FORMAT_DIRECT_REG_LIST_LENGTH];
        format.extend_from_slice(&[0x1234, 0, 7]);
        format.push(0xFFFF_FFFF);
        format.extend_from_slice(&[0x5678, 0, 7]);
        format.push(0xFFFF_FFFF);
        format.extend_from_slice(&[0x9ABC, 0, 9]);
        let parsed = parse_indirect_list(&mut format).unwrap();
        assert_eq!(parsed.start_count, 3);
        assert_eq!(parsed.starts[0], 14);
        assert_eq!(parsed.starts[1], 18);
        assert_eq!(parsed.starts[2], 22);
        assert_eq!(parsed.unique_count, 2);
        assert_eq!(parsed.unique[0], 7);
        assert_eq!(parsed.unique[1], 9);
        assert_eq!(format[16], 0);
        assert_eq!(format[20], 0);
        assert_eq!(format[24], 1);
    }

    #[test]
    fn indirect_list_rejects_overflow() {
        let mut format = vec![0xFFFF_FFFF; 40];
        assert!(parse_indirect_list(&mut format).is_err());
    }

    #[test]
    fn save_restore_list_streams_aram_and_scratch() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        let list = RlcRegisterList {
            format: vec![0u32; FORMAT_DIRECT_REG_LIST_LENGTH]
                .into_iter()
                .chain([0x1234, 0, 0x0030_0007].to_vec())
                .collect(),
            restore: vec![0xAAAA_0001, 0xAAAA_0002, 0xAAAA_0003, 0xAAAA_0004],
            format_start: 0x40,
            restore_list_size: 9,
            offsets_start: 0x50,
        };
        init_save_restore_list(&mmio, &list).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (RLC_SRM_CNTL as usize * 4, SRM_AUTO_INCR_ADDR));
        assert_eq!(writes[1], (RLC_SRM_ARAM_ADDR as usize * 4, 0));
        let aram: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SRM_ARAM_DATA as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(aram, list.restore);
        let scratch_addrs: Vec<(usize, u32)> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_GPM_SCRATCH_ADDR as usize * 4)
            .map(|entry| (entry.0, entry.1))
            .collect();
        assert_eq!(
            scratch_addrs,
            vec![
                (RLC_GPM_SCRATCH_ADDR as usize * 4, 0x40),
                (RLC_GPM_SCRATCH_ADDR as usize * 4, 9),
                (RLC_GPM_SCRATCH_ADDR as usize * 4, 0x50)
            ]
        );
        let scratch_data: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_GPM_SCRATCH_DATA as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(scratch_data.len(), 17 + 1 + 10);
        assert_eq!(scratch_data[17], 2);
        assert_eq!(scratch_data[18], 14);
        assert_eq!(scratch_data[19], 0);
        let cntl_addr: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SRM_INDEX_CNTL_ADDR_0 as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(cntl_addr, vec![0x0030_0007 & SRM_INDEX_RANGE_MASK]);
        let cntl_data: Vec<u32> = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SRM_INDEX_CNTL_DATA_0 as usize * 4)
            .map(|entry| entry.1)
            .collect();
        assert_eq!(cntl_data, vec![0x0030_0007 >> 20]);
    }

    #[test]
    fn init_pg_programs_csb_srm_and_delays() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_SRM_CNTL as usize] = 0x10;
        mmio.bar().cells.borrow_mut()[RLC_PG_DELAY_2 as usize] = 0x1234_5678;
        mmio.bar().cells.borrow_mut()[RLC_AUTO_PG_CTRL as usize] = 0xFFFF_0000;
        mmio.bar().cells.borrow_mut()[CP_RB_WPTR_POLL_CNTL as usize] = 0x0001_0000;
        let list = RlcRegisterList {
            format: vec![0; FORMAT_DIRECT_REG_LIST_LENGTH],
            restore: vec![1, 2, 3, 4],
            format_start: 0x40,
            restore_list_size: 5,
            offsets_start: 0x50,
        };
        let csb = [0xC000_4A00u32, 1, 0xC001_2800, 0x8000_0000, 0x8000_0000];
        let setup = PgSetup {
            csb: &csb,
            csb_va: 0x412_000,
            list: &list,
        };
        init_pg(&mmio, &setup).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (RLC_CSIB_ADDR_HI as usize * 4, 0));
        assert_eq!(writes[1], (RLC_CSIB_ADDR_LO as usize * 4, 0x412_000));
        assert_eq!(writes[2], (RLC_CSIB_LENGTH as usize * 4, 5));
        let cntl = writes
            .iter()
            .filter(|entry| entry.0 == RLC_SRM_CNTL as usize * 4)
            .map(|entry| entry.1)
            .collect::<Vec<_>>();
        assert_eq!(cntl, vec![0x12, 0x13]);
        let delay = writes
            .iter()
            .find(|entry| entry.0 == RLC_PG_DELAY as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(delay.1, 0x1010_1010);
        let delay2 = writes
            .iter()
            .find(|entry| entry.0 == RLC_PG_DELAY_2 as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(
            delay2.1,
            0x1234_5678 & !PG_DELAY_2_SERDES_CMD_DELAY_MASK | PG_DELAY_2_SERDES_CMD_DELAY
        );
        let auto_ctrl = writes
            .iter()
            .find(|entry| entry.0 == RLC_AUTO_PG_CTRL as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(
            auto_ctrl.1,
            (0xFFFF_0000 & !AUTO_PG_CTRL_IDLE_THRESHOLD_MASK) | AUTO_PG_CTRL_IDLE_THRESHOLD
        );
        let poll = writes
            .iter()
            .find(|entry| entry.0 == CP_RB_WPTR_POLL_CNTL as usize * 4)
            .copied()
            .unwrap();
        assert_eq!(
            poll.1,
            0x0001_0000 & !WPTR_POLL_IDLE_COUNT_MASK | WPTR_POLL_IDLE_COUNT
        );
    }

    #[test]
    fn pg_enable_toggles_static_and_dyn_bits() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[RLC_PG_CNTL as usize] = 0x40;
        enable_pg(&mmio);
        assert_eq!(
            mmio.bar().cells.borrow()[RLC_PG_CNTL as usize],
            0x40 | PG_CNTL_STATIC_PER_CU_PG | PG_CNTL_DYN_PER_CU_PG
        );
        disable_pg(&mmio);
        assert_eq!(mmio.bar().cells.borrow()[RLC_PG_CNTL as usize], 0x40);
    }
}
