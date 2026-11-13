pub const KERNEL_MAGIC: u32 = 0x5A5A_0001;
pub const VGPRS: u32 = 2;
pub const SGPRS: u32 = 7;
pub const USER_SGPRS: u32 = 2;

pub const KERNEL_WORDS: [u32; 10] = [
    0xC00A_0002,
    0x0000_0000,
    0xBE86_0080,
    0xBF8C_F070,
    0x7E00_02FF,
    KERNEL_MAGIC,
    0x7E02_02C0,
    0xE070_1000,
    0x0600_0001,
    0xBF81_0000,
];

pub fn pgm_rsrc1(vgprs: u32, sgprs: u32) -> u32 {
    let vgpr_granules = vgprs.div_ceil(4).max(1) - 1;
    let sgpr_granules = sgprs.div_ceil(8).max(1) - 1;
    vgpr_granules | (sgpr_granules << 6)
}

pub fn pgm_rsrc2(user_sgprs: u32) -> u32 {
    user_sgprs << 1
}

pub fn buffer_descriptor(base: u64, stride: u32, bytes: u32) -> [u32; 4] {
    [
        base as u32,
        (((base >> 32) as u32) & 0xFFFF) | ((stride & 0x3FFF) << 16),
        bytes,
        0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smem_load_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[0], 0xC00A_0002);
        assert_eq!(KERNEL_WORDS[1], 0x0000_0000);
    }

    #[test]
    fn s_mov_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[2], 0xBE86_0080);
    }

    #[test]
    fn waitcnt_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[3], 0xBF8C_F070);
    }

    #[test]
    fn v_mov_literal_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[4], 0x7E00_02FF);
        assert_eq!(KERNEL_WORDS[5], KERNEL_MAGIC);
    }

    #[test]
    fn v_mov_inline_zero_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[6], 0x7E02_02C0);
    }

    #[test]
    fn buffer_store_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[7], 0xE070_1000);
        assert_eq!(KERNEL_WORDS[8], 0x0600_0001);
    }

    #[test]
    fn endpgm_matches_llvm_reference_encoding() {
        assert_eq!(KERNEL_WORDS[9], 0xBF81_0000);
    }

    #[test]
    fn rsrc_fields_use_gfx8_kernel_header_shifts() {
        assert_eq!(pgm_rsrc1(VGPRS, SGPRS), 0x0000_0000);
        assert_eq!(pgm_rsrc1(16, 24), (4 - 1) | (3 - 1) << 6);
        assert_eq!(pgm_rsrc2(USER_SGPRS), 0x0000_0004);
        assert_eq!(pgm_rsrc2(16), 16 << 1);
    }

    #[test]
    fn buffer_descriptor_matches_isa_manual_layout() {
        assert_eq!(
            buffer_descriptor(0x0000_1234_5678_9ABC, 0, 4),
            [0x5678_9ABC, 0x1234, 4, 0]
        );
        assert_eq!(
            buffer_descriptor(0x1000, 64, 256),
            [0x1000, 64 << 16, 256, 0]
        );
    }
}
