use std::io;

use ferens_core::{BarOps, Chip, Mmio};

use crate::golden_data::{
    GFX_POLARIS10, GFX_POLARIS10_COMMON, GFX_POLARIS11, GFX_POLARIS11_COMMON, GMC_POLARIS10,
    GMC_POLARIS11,
};

pub const SMC_IND_INDEX: u32 = 0x80;
pub const SMC_IND_DATA: u32 = 0x81;
pub const CG_ACLK_CNTL: u32 = 0xc050_00dc;
pub const CG_ACLK_DIVIDER_MASK: u32 = 0x7f;
pub const CG_ACLK_DIVIDER_POLARIS10: u32 = 0x18;

#[derive(Clone, Copy)]
pub struct GoldenEntry {
    pub offset: u32,
    pub and_mask: u32,
    pub or_mask: u32,
}

pub fn program<B: BarOps>(mmio: &Mmio<B>, table: &[GoldenEntry]) {
    for entry in table {
        let value = if entry.and_mask == u32::MAX {
            entry.or_mask
        } else {
            (mmio.read_reg_direct(entry.offset) & !entry.and_mask) | entry.or_mask
        };
        mmio.write_reg_direct(entry.offset, value);
    }
}

pub fn smc_read<B: BarOps>(mmio: &Mmio<B>, index: u32) -> u32 {
    mmio.write_reg_direct(SMC_IND_INDEX, index);
    mmio.read_reg_direct(SMC_IND_DATA)
}

pub fn smc_write<B: BarOps>(mmio: &Mmio<B>, index: u32, value: u32) {
    mmio.write_reg_direct(SMC_IND_INDEX, index);
    mmio.write_reg_direct(SMC_IND_DATA, value);
}

pub fn apply<B: BarOps>(mmio: &Mmio<B>, chip: Chip) -> io::Result<()> {
    match chip {
        Chip::Polaris10 => {
            program(mmio, GMC_POLARIS10);
            program(mmio, GFX_POLARIS10);
            program(mmio, GFX_POLARIS10_COMMON);
            let aclk = smc_read(mmio, CG_ACLK_CNTL);
            smc_write(
                mmio,
                CG_ACLK_CNTL,
                (aclk & !CG_ACLK_DIVIDER_MASK) | CG_ACLK_DIVIDER_POLARIS10,
            );
        }
        Chip::Polaris11 | Chip::Polaris12 => {
            program(mmio, GMC_POLARIS11);
            program(mmio, GFX_POLARIS11);
            program(mmio, GFX_POLARIS11_COMMON);
        }
        _ => {
            return Err(io::Error::other(
                "golden registers are known for polaris10, polaris11 and polaris12",
            ))
        }
    }
    Ok(())
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
    fn program_rmw_keeps_masked_out_bits() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        mmio.bar().cells.borrow_mut()[0x2684] = 0xffff_ffff;
        program(
            &mmio,
            &[GoldenEntry {
                offset: 0x2684,
                and_mask: 0x1f3cf,
                or_mask: 0x7208,
            }],
        );
        assert_eq!(mmio.bar().cells.borrow()[0x2684], 0xfffe_7e38);
    }

    #[test]
    fn program_full_mask_writes_without_read() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        program(
            &mmio,
            &[GoldenEntry {
                offset: 0x22fc,
                and_mask: 0xffff_ffff,
                or_mask: 0x2000_0001,
            }],
        );
        assert_eq!(
            mmio.bar().writes.borrow().as_slice(),
            &[(0x22fc_usize * 4, 0x2000_0001)]
        );
    }

    #[test]
    fn apply_covers_both_chip_families() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        apply(&mmio, Chip::Polaris10).unwrap();
        let gmc_writes = mmio.bar().writes.borrow();
        let prt = GMC_POLARIS10
            .iter()
            .filter(|entry| entry.offset == 0x52c)
            .count();
        assert_eq!(prt, 1);
        assert!(gmc_writes.len() >= 2);
        drop(gmc_writes);
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        apply(&mmio, Chip::Polaris12).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (0x52c_usize * 4, 0x0fff_ffff));
        let smc_offsets = [SMC_IND_INDEX as usize * 4, SMC_IND_DATA as usize * 4];
        assert!(writes.iter().all(|entry| !smc_offsets.contains(&entry.0)));
    }

    #[test]
    fn apply_writes_polaris10_aclk_divider_via_smc_window() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        apply(&mmio, Chip::Polaris10).unwrap();
        let writes = mmio.bar().writes.borrow();
        let smc: Vec<(usize, u32)> = writes
            .iter()
            .filter(|entry| {
                entry.0 == SMC_IND_INDEX as usize * 4 || entry.0 == SMC_IND_DATA as usize * 4
            })
            .copied()
            .collect();
        assert_eq!(
            smc,
            vec![
                (SMC_IND_INDEX as usize * 4, CG_ACLK_CNTL),
                (SMC_IND_INDEX as usize * 4, CG_ACLK_CNTL),
                (SMC_IND_DATA as usize * 4, CG_ACLK_DIVIDER_POLARIS10),
            ]
        );
    }

    #[test]
    fn apply_rejects_unknown_chip() {
        let mmio = Mmio::new(FakeBar::new(BAR_WORDS));
        assert!(apply(&mmio, Chip::VegaM).is_err());
        assert!(apply(&mmio, Chip::Unknown(0x1234)).is_err());
    }

    #[test]
    fn golden_tables_match_kernel_sources() {
        assert_eq!(GFX_POLARIS10.len(), 17);
        assert_eq!(GFX_POLARIS10_COMMON.len(), 8);
        assert_eq!(GFX_POLARIS11.len(), 17);
        assert_eq!(GFX_POLARIS11_COMMON.len(), 6);
        assert_eq!(GMC_POLARIS10.len(), 5);
        assert_eq!(GMC_POLARIS11.len(), 4);
        let rlc = GFX_POLARIS10
            .iter()
            .find(|entry| entry.offset == crate::rlc::RLC_CGCG_CGLS_CTRL)
            .unwrap();
        assert_eq!((rlc.and_mask, rlc.or_mask), (0x3, 0x1_003c));
    }
}
