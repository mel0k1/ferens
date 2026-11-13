use std::io;

use ferens_core::{BarOps, MappedBar, Mmio};

pub const VM_L2_CNTL: u32 = 0x500;
pub const VM_L2_CNTL2: u32 = 0x501;
pub const VM_L2_CNTL3: u32 = 0x502;
pub const VM_CONTEXT0_CNTL: u32 = 0x504;
pub const VM_CONTEXT0_CNTL2: u32 = 0x50C;
pub const VM_INVALIDATE_REQUEST: u32 = 0x51E;
pub const VM_INVALIDATE_RESPONSE: u32 = 0x51F;
pub const VM_CONTEXT0_PROTECTION_FAULT_DEFAULT_ADDR: u32 = 0x546;
pub const VM_CONTEXT0_PAGE_TABLE_BASE_ADDR: u32 = 0x54F;
pub const VM_CONTEXT0_PAGE_TABLE_START_ADDR: u32 = 0x557;
pub const VM_CONTEXT0_PAGE_TABLE_END_ADDR: u32 = 0x55F;
pub const VM_L2_CNTL4: u32 = 0x578;
pub const MC_VM_MX_L1_TLB_CNTL: u32 = 0x819;

pub const MC_L1_ENABLE_L1_TLB: u32 = 1;
pub const MC_L1_ENABLE_FRAGMENT_PROCESSING: u32 = 1 << 1;
pub const MC_L1_SYSTEM_ACCESS_MODE_MASK: u32 = 0x18;
pub const MC_L1_SYSTEM_ACCESS_MODE_PASS_THROUGH: u32 = 3 << 3;
pub const MC_L1_UNMAPPED_ACCESS_MASK: u32 = 0x20;
pub const MC_L1_UNMAPPED_DEFAULT_PAGE: u32 = 0;
pub const MC_L1_ENABLE_ADVANCED_DRIVER_MODEL: u32 = 1 << 6;

pub const L2_ENABLE_L2_CACHE: u32 = 1;
pub const L2_ENABLE_FRAGMENT_PROCESSING: u32 = 1 << 1;
pub const L2_PTE_LRU_UPDATE_BY_WRITE: u32 = 1 << 9;
pub const L2_PDE0_LRU_UPDATE_BY_WRITE: u32 = 1 << 10;
pub const L2_DEFAULT_PAGE_OUT_TO_SYSTEM: u32 = 1 << 11;
pub const L2_EFFECTIVE_QUEUE_SIZE: u32 = 7 << 15;
pub const L2_CONTEXT1_IDENTITY_ACCESS_MODE: u32 = 1 << 19;
pub const L2_INVALIDATE_ALL_L1_TLBS: u32 = 1;
pub const L2_INVALIDATE_L2_CACHE: u32 = 1 << 1;
pub const L2_BANK_SELECT: u32 = 4;
pub const L2_BANK_SELECT_MASK: u32 = 0x3F;
pub const L2_BIGK_FRAGMENT_SIZE: u32 = 4;
pub const L2_BIGK_FRAGMENT_SIZE_MASK: u32 = 0xF8000;
pub const L2_BIGK_ASSOCIATIVITY: u32 = 1 << 20;
pub const L2_CNTL4_TAP_REQUEST_MASK: u32 = 0x3FFC0;

pub const VM_ENABLE_CONTEXT: u32 = 1;
pub const VM_PAGE_TABLE_DEPTH_FLAT: u32 = 0;
pub const VM_PAGE_TABLE_DEPTH_MASK: u32 = 0x6;
pub const VM_RANGE_PROTECTION_FAULT_ENABLE_DEFAULT: u32 = 1 << 4;

pub const PTE_VALID: u64 = 1;
pub const PTE_SNOOPED: u64 = 1 << 2;
pub const PTE_EXECUTABLE: u64 = 1 << 4;
pub const PTE_READABLE: u64 = 1 << 5;
pub const PTE_WRITEABLE: u64 = 1 << 6;
pub const PTE_FLAGS: u64 = PTE_VALID | PTE_SNOOPED | PTE_EXECUTABLE | PTE_READABLE | PTE_WRITEABLE;
pub const PTE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

pub const GPU_PAGE_BYTES: u64 = 4096;

pub struct VmConfig {
    pub page_table_base_pfn: u32,
    pub start_pfn: u32,
    pub end_pfn: u32,
    pub default_page_pfn: u32,
    pub pages: usize,
}

pub fn pte_identity_word(page_addr: u64) -> u64 {
    (page_addr & PTE_ADDR_MASK) | PTE_FLAGS
}

pub fn pte_page_count(start_pfn: u32, end_pfn: u32) -> usize {
    (end_pfn - start_pfn + 1) as usize
}

pub fn fill_identity_table(
    aperture: &mut MappedBar,
    table_byte_offset: usize,
    start_pfn: u32,
    pages: usize,
) -> io::Result<()> {
    let words = aperture.words_mut();
    let first_word = table_byte_offset / 4;
    let last_word = first_word + pages * 2;
    if last_word > words.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "page table does not fit in aperture",
        ));
    }
    for index in 0..pages {
        let page_addr = start_pfn as u64 * GPU_PAGE_BYTES + index as u64 * GPU_PAGE_BYTES;
        let entry = pte_identity_word(page_addr);
        words[first_word + index * 2] = entry as u32;
        words[first_word + index * 2 + 1] = (entry >> 32) as u32;
    }
    Ok(())
}

pub fn program_l1_l2<B: BarOps>(mmio: &Mmio<B>) {
    let mut value = mmio.read_reg_direct(MC_VM_MX_L1_TLB_CNTL);
    value &= !(MC_L1_SYSTEM_ACCESS_MODE_MASK | MC_L1_UNMAPPED_ACCESS_MASK);
    value |= MC_L1_ENABLE_L1_TLB
        | MC_L1_ENABLE_FRAGMENT_PROCESSING
        | MC_L1_SYSTEM_ACCESS_MODE_PASS_THROUGH
        | MC_L1_ENABLE_ADVANCED_DRIVER_MODEL;
    mmio.write_reg_direct(MC_VM_MX_L1_TLB_CNTL, value);

    let mut value = mmio.read_reg_direct(VM_L2_CNTL);
    value |= L2_ENABLE_L2_CACHE
        | L2_ENABLE_FRAGMENT_PROCESSING
        | L2_PTE_LRU_UPDATE_BY_WRITE
        | L2_PDE0_LRU_UPDATE_BY_WRITE
        | L2_DEFAULT_PAGE_OUT_TO_SYSTEM
        | L2_EFFECTIVE_QUEUE_SIZE
        | L2_CONTEXT1_IDENTITY_ACCESS_MODE;
    mmio.write_reg_direct(VM_L2_CNTL, value);

    let mut value = mmio.read_reg_direct(VM_L2_CNTL2);
    value |= L2_INVALIDATE_ALL_L1_TLBS | L2_INVALIDATE_L2_CACHE;
    mmio.write_reg_direct(VM_L2_CNTL2, value);

    let mut value = mmio.read_reg_direct(VM_L2_CNTL3);
    value &= !(L2_BANK_SELECT_MASK | L2_BIGK_FRAGMENT_SIZE_MASK);
    value |= L2_BANK_SELECT | (L2_BIGK_FRAGMENT_SIZE << 15) | L2_BIGK_ASSOCIATIVITY;
    mmio.write_reg_direct(VM_L2_CNTL3, value);

    let mut value = mmio.read_reg_direct(VM_L2_CNTL4);
    value &= !L2_CNTL4_TAP_REQUEST_MASK;
    mmio.write_reg_direct(VM_L2_CNTL4, value);
}

pub fn program_context0<B: BarOps>(mmio: &Mmio<B>, config: &VmConfig) {
    mmio.write_reg_direct(VM_CONTEXT0_PAGE_TABLE_START_ADDR, config.start_pfn);
    mmio.write_reg_direct(VM_CONTEXT0_PAGE_TABLE_END_ADDR, config.end_pfn);
    mmio.write_reg_direct(VM_CONTEXT0_PAGE_TABLE_BASE_ADDR, config.page_table_base_pfn);
    mmio.write_reg_direct(
        VM_CONTEXT0_PROTECTION_FAULT_DEFAULT_ADDR,
        config.default_page_pfn,
    );
    mmio.write_reg_direct(VM_CONTEXT0_CNTL2, 0);

    let mut value = mmio.read_reg_direct(VM_CONTEXT0_CNTL);
    value |= VM_ENABLE_CONTEXT
        | (VM_PAGE_TABLE_DEPTH_FLAT << 1)
        | VM_RANGE_PROTECTION_FAULT_ENABLE_DEFAULT;
    value &= !VM_PAGE_TABLE_DEPTH_MASK;
    mmio.write_reg_direct(VM_CONTEXT0_CNTL, value);
}

pub fn invalidate<B: BarOps>(mmio: &Mmio<B>, vmid: u32) {
    mmio.write_reg_direct(VM_INVALIDATE_REQUEST, 1 << vmid);
    let _ = mmio.read_reg_direct(VM_INVALIDATE_RESPONSE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pte_identity_keeps_page_alignment() {
        let entry = pte_identity_word(0x0400_0000);
        assert_eq!(entry, 0x0400_0000 | PTE_FLAGS);
        assert_eq!(pte_identity_word(0x0400_1004), 0x0400_1000 | PTE_FLAGS);
        assert_eq!(
            PTE_FLAGS,
            (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6)
        );
    }

    #[test]
    fn page_count_is_inclusive() {
        assert_eq!(pte_page_count(0x4000, 0x4FFF), 4096);
        assert_eq!(pte_page_count(7, 7), 1);
    }

    #[test]
    fn config_covers_vram_window() {
        let config = VmConfig {
            page_table_base_pfn: 0x4002,
            start_pfn: 0x4000,
            end_pfn: 0x4FFF,
            default_page_pfn: 0x400C,
            pages: 4096,
        };
        assert_eq!(config.start_pfn, 0x0400_0000 >> 12);
        assert_eq!(config.end_pfn, config.start_pfn + config.pages as u32 - 1);
        assert_eq!(config.page_table_base_pfn, (0x0400_0000 + 0x2000) >> 12);
    }
}
