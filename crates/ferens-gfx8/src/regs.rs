pub const MM_INDEX: u32 = 0x0000;
pub const MM_DATA: u32 = 0x0001;
pub const CONFIG_MEMSIZE: u32 = 0x150A;
pub const GRBM_GFX_INDEX: u32 = 0xC200;

pub const SET_SH_REG_START: u32 = 0x2C00;
pub const SET_CONTEXT_REG_START: u32 = 0xA000;

pub const COMPUTE_DISPATCH_INITIATOR: u32 = 0x2E00;
pub const COMPUTE_DIM_X: u32 = 0x2E01;
pub const COMPUTE_DIM_Y: u32 = 0x2E02;
pub const COMPUTE_DIM_Z: u32 = 0x2E03;
pub const COMPUTE_START_X: u32 = 0x2E04;
pub const COMPUTE_START_Y: u32 = 0x2E05;
pub const COMPUTE_START_Z: u32 = 0x2E06;
pub const COMPUTE_NUM_THREAD_X: u32 = 0x2E07;
pub const COMPUTE_NUM_THREAD_Y: u32 = 0x2E08;
pub const COMPUTE_NUM_THREAD_Z: u32 = 0x2E09;
pub const COMPUTE_PGM_LO: u32 = 0x2E0C;
pub const COMPUTE_PGM_HI: u32 = 0x2E0D;
pub const COMPUTE_PGM_RSRC1: u32 = 0x2E12;
pub const COMPUTE_PGM_RSRC2: u32 = 0x2E13;
pub const COMPUTE_USER_DATA_0: u32 = 0x2E40;
pub const COMPUTE_USER_DATA_15: u32 = 0x2E4F;

pub const DISPATCH_SHADER_EN: u32 = 1 << 0;

pub fn sh_offset(mm_offset: u32) -> u32 {
    mm_offset - SET_SH_REG_START
}

pub fn ctx_offset(mm_offset: u32) -> u32 {
    mm_offset - SET_CONTEXT_REG_START
}

pub fn user_data(index: u32) -> Option<u32> {
    if index <= 15 {
        Some(COMPUTE_USER_DATA_0 + index)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_offset_matches_kernel_convention() {
        assert_eq!(sh_offset(COMPUTE_PGM_LO), 0x20C);
        assert_eq!(sh_offset(COMPUTE_PGM_RSRC1), 0x212);
        assert_eq!(sh_offset(COMPUTE_USER_DATA_0), 0x240);
    }

    #[test]
    fn user_data_range_is_bounded() {
        assert_eq!(user_data(15), Some(0x2E4F));
        assert_eq!(user_data(16), None);
    }
}
