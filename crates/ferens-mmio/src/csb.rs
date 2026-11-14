use ferens_gfx8::op;
use ferens_gfx8::pm4::type3_header;
use ferens_gfx8::regs;

use crate::csb_data::{CSB_EXTENTS, CSB_EXTENT_DWORDS};

pub const PREAMBLE_BEGIN_CLEAR_STATE: u32 = 1;
pub const PREAMBLE_END_CLEAR_STATE: u32 = 0;
pub const CONTEXT_CONTROL_ENABLE: u32 = 0x8000_0000;

pub const CSB_HEADER_WORDS: usize = 2;
pub const CSB_CONTEXT_CONTROL_WORDS: usize = 3;
pub const CSB_RASTER_WORDS: usize = 4;
pub const CSB_TAIL_WORDS: usize = 4;
pub const CSB_WORDS: usize = CSB_HEADER_WORDS
    + CSB_CONTEXT_CONTROL_WORDS
    + CSB_EXTENT_DWORDS
    + 2 * CSB_EXTENTS.len()
    + CSB_RASTER_WORDS
    + CSB_TAIL_WORDS;

pub fn csb_words(raster_config: u32, raster_config_1: u32) -> Vec<u32> {
    let mut words = Vec::with_capacity(CSB_WORDS);
    words.push(type3_header(op::PREAMBLE_CNTL, 0));
    words.push(PREAMBLE_BEGIN_CLEAR_STATE);
    words.push(type3_header(op::CONTEXT_CONTROL, 1));
    words.push(CONTEXT_CONTROL_ENABLE);
    words.push(CONTEXT_CONTROL_ENABLE);
    for extent in &CSB_EXTENTS {
        words.push(type3_header(
            op::SET_CONTEXT_REG,
            extent.values.len() as u32,
        ));
        words.push(extent.reg_index - regs::SET_CONTEXT_REG_START);
        words.extend_from_slice(extent.values);
    }
    words.push(type3_header(op::SET_CONTEXT_REG, 2));
    words.push(regs::ctx_offset(regs::PA_SC_RASTER_CONFIG));
    words.push(raster_config);
    words.push(raster_config_1);
    words.push(type3_header(op::PREAMBLE_CNTL, 0));
    words.push(PREAMBLE_END_CLEAR_STATE);
    words.push(type3_header(op::CLEAR_STATE, 0));
    words.push(0);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csb_size_matches_kernel_get_csb_size() {
        let count = 2
            + 3
            + CSB_EXTENTS
                .iter()
                .map(|extent| 2 + extent.values.len())
                .sum::<usize>()
            + 4
            + 2
            + 2;
        assert_eq!(CSB_WORDS, count);
        assert_eq!(CSB_EXTENT_DWORDS, 885);
    }

    #[test]
    fn csb_words_follow_get_csb_buffer_layout() {
        let words = csb_words(0x1600_0012, 0x2E);
        assert_eq!(words.len(), CSB_WORDS);
        assert_eq!(words[0], 0xC000_4A00);
        assert_eq!(words[1], PREAMBLE_BEGIN_CLEAR_STATE);
        assert_eq!(words[2], 0xC001_2800);
        assert_eq!(words[3], CONTEXT_CONTROL_ENABLE);
        assert_eq!(words[4], CONTEXT_CONTROL_ENABLE);
        let mut cursor = 5;
        for extent in &CSB_EXTENTS {
            let header = 0xC000_0000
                | (((extent.values.len() as u32) & 0x3FFF) << 16)
                | ((op::SET_CONTEXT_REG as u32) << 8);
            assert_eq!(words[cursor], header);
            assert_eq!(
                words[cursor + 1],
                extent.reg_index - regs::SET_CONTEXT_REG_START
            );
            cursor += 2 + extent.values.len();
        }
        assert_eq!(words[cursor], 0xC002_6900);
        assert_eq!(
            words[cursor + 1],
            regs::ctx_offset(regs::PA_SC_RASTER_CONFIG)
        );
        assert_eq!(words[cursor + 2], 0x1600_0012);
        assert_eq!(words[cursor + 3], 0x2E);
        cursor += 4;
        assert_eq!(words[cursor], 0xC000_4A00);
        assert_eq!(words[cursor + 1], PREAMBLE_END_CLEAR_STATE);
        assert_eq!(words[cursor + 2], 0xC000_1200);
        assert_eq!(words[cursor + 3], 0);
        cursor += 4;
        assert_eq!(cursor, words.len());
    }

    #[test]
    fn extent_reg_indexes_are_context_window_offsets() {
        assert_eq!(CSB_EXTENTS[0].reg_index, 0xA000);
        assert_eq!(CSB_EXTENTS[0].values.len(), 212);
        assert_eq!(CSB_EXTENTS[1].reg_index, 0xA0D6);
        assert_eq!(CSB_EXTENTS[6].reg_index, 0xA2A5);
        assert_eq!(CSB_EXTENTS[6].values.len(), 233);
        assert_eq!(regs::ctx_offset(regs::PA_SC_RASTER_CONFIG), 0xD4);
    }
}
