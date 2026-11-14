use crate::op;
use crate::regs;

pub const PACKET_TYPE3: u32 = 3;
pub const COMPUTE_HEADER_FLAG: u32 = 1 << 1;
pub const INDIRECT_BUFFER_VALID: u32 = 1 << 23;
pub const PACKET_COUNT_MAX: u32 = 0x3FFF;

pub const NOP_WORD: u32 = type3_header(op::NOP, 0);

pub const fn type3_header(opcode: u8, count: u32) -> u32 {
    (PACKET_TYPE3 << 30) | ((count & PACKET_COUNT_MAX) << 16) | ((opcode as u32) << 8)
}

pub const fn compute_header(opcode: u8, count: u32) -> u32 {
    type3_header(opcode, count) | COMPUTE_HEADER_FLAG
}

pub const fn event_word(event_type: u32, event_index: u32) -> u32 {
    (event_type << op::EVENT_TYPE_SHIFT) | (event_index << op::EVENT_INDEX_SHIFT)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaitRegMem {
    pub operation: u32,
    pub function: u32,
    pub engine: u32,
    pub addr: u64,
    pub reference: u32,
    pub mask: u32,
    pub poll_interval: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketBuilder {
    words: Vec<u32>,
}

impl PacketBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.words
    }

    pub fn into_words(self) -> Vec<u32> {
        self.words
    }

    pub fn nop(&mut self, filler_words: usize) {
        assert!(
            filler_words as u32 <= PACKET_COUNT_MAX,
            "packet too large for header"
        );
        self.words.push(type3_header(op::NOP, filler_words as u32));
        let end = self.words.len() + filler_words;
        self.words.resize(end, 0);
    }

    pub fn set_sh_reg(&mut self, sh_offset: u32, values: &[u32]) {
        assert!(
            values.len() as u32 <= PACKET_COUNT_MAX,
            "packet too large for header"
        );
        self.words
            .push(type3_header(op::SET_SH_REG, values.len() as u32));
        self.words.push(sh_offset);
        self.words.extend_from_slice(values);
    }

    pub fn set_context_reg(&mut self, ctx_offset: u32, values: &[u32]) {
        assert!(
            values.len() as u32 <= PACKET_COUNT_MAX,
            "packet too large for header"
        );
        self.words
            .push(type3_header(op::SET_CONTEXT_REG, values.len() as u32));
        self.words.push(ctx_offset);
        self.words.extend_from_slice(values);
    }

    pub fn dispatch_direct(&mut self, dim_x: u32, dim_y: u32, dim_z: u32, initiator: u32) {
        self.words.push(type3_header(op::DISPATCH_DIRECT, 3));
        self.words.push(dim_x);
        self.words.push(dim_y);
        self.words.push(dim_z);
        self.words.push(initiator);
    }

    pub fn compute_dispatch(&mut self, dim_x: u32, dim_y: u32, dim_z: u32) {
        self.dispatch_direct(dim_x, dim_y, dim_z, regs::DISPATCH_SHADER_EN);
    }

    pub fn indirect_buffer(&mut self, addr: u64, size_dw: u32, vmid: u8) {
        self.push_ib(op::INDIRECT_BUFFER, addr, size_dw, vmid, false);
    }

    pub fn indirect_buffer_valid(&mut self, addr: u64, size_dw: u32, vmid: u8) {
        self.push_ib(op::INDIRECT_BUFFER, addr, size_dw, vmid, true);
    }

    pub fn indirect_buffer_const(&mut self, addr: u64, size_dw: u32, vmid: u8) {
        self.push_ib(op::INDIRECT_BUFFER_CONST, addr, size_dw, vmid, false);
    }

    fn push_ib(&mut self, opcode: u8, addr: u64, size_dw: u32, vmid: u8, valid: bool) {
        self.words.push(type3_header(opcode, 2));
        self.words.push((addr & !0x3) as u32);
        self.words.push(((addr >> 32) & 0xFFFF) as u32);
        let mut control = size_dw | (u32::from(vmid) << 24);
        if valid {
            control |= INDIRECT_BUFFER_VALID;
        }
        self.words.push(control);
    }

    pub fn write_data(
        &mut self,
        dst_sel: u32,
        engine_sel: u32,
        confirm: bool,
        addr: u64,
        data: &[u32],
    ) {
        assert!(
            data.len() as u32 <= PACKET_COUNT_MAX - 2,
            "packet too large for header"
        );
        self.words
            .push(type3_header(op::WRITE_DATA, 2 + data.len() as u32));
        let mut control = (dst_sel << 8) | (engine_sel << 30);
        if confirm {
            control |= op::WRITE_DATA_WR_CONFIRM;
        }
        self.words.push(control);
        self.words.push(addr as u32);
        self.words.push((addr >> 32) as u32);
        self.words.extend_from_slice(data);
    }

    pub fn wait_reg_mem(&mut self, spec: WaitRegMem) {
        self.words.push(type3_header(op::WAIT_REG_MEM, 5));
        self.words
            .push(spec.function | (spec.operation << 6) | (spec.engine << 8));
        self.words.push(spec.addr as u32);
        self.words.push((spec.addr >> 32) as u32);
        self.words.push(spec.reference);
        self.words.push(spec.mask);
        self.words.push(spec.poll_interval);
    }

    pub fn event_write(&mut self, word: u32) {
        self.words.push(type3_header(op::EVENT_WRITE, 0));
        self.words.push(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_matches_kernel_macro() {
        assert_eq!(type3_header(op::SET_SH_REG, 1), 0xC001_7600);
        assert_eq!(type3_header(op::DISPATCH_DIRECT, 3), 0xC003_1500);
        assert_eq!(type3_header(op::NOP, 0), 0xC000_1000);
        assert_eq!(compute_header(op::DISPATCH_DIRECT, 3), 0xC003_1502);
        assert_eq!(NOP_WORD, 0xC000_1000);
    }

    #[test]
    fn set_sh_reg_puts_offset_first_then_values() {
        let mut builder = PacketBuilder::new();
        builder.set_sh_reg(0x20C, &[0xAABB, 0xCCDD]);
        assert_eq!(builder.as_slice(), &[0xC002_7600, 0x20C, 0xAABB, 0xCCDD]);
    }

    #[test]
    fn dispatch_direct_body_is_xyz_then_initiator() {
        let mut builder = PacketBuilder::new();
        builder.dispatch_direct(8, 1, 1, 1);
        assert_eq!(builder.as_slice(), &[0xC003_1500, 8, 1, 1, 1]);
    }

    #[test]
    fn write_data_matches_kernel_fence_layout() {
        let mut builder = PacketBuilder::new();
        builder.write_data(
            op::WRITE_DATA_DST_SEL_MEMORY,
            op::WRITE_DATA_ENGINE_ME,
            true,
            0x1_0000_0000,
            &[0xDEAD_BEEF],
        );
        let control = (op::WRITE_DATA_DST_SEL_MEMORY << 8)
            | (op::WRITE_DATA_ENGINE_ME << 30)
            | op::WRITE_DATA_WR_CONFIRM;
        assert_eq!(
            builder.as_slice(),
            &[0xC003_3700, control, 0, 0x1, 0xDEAD_BEEF]
        );
    }

    #[test]
    fn wait_reg_mem_matches_kernel_flush_layout() {
        let mut builder = PacketBuilder::new();
        builder.wait_reg_mem(WaitRegMem {
            operation: 0,
            function: op::WAIT_REG_MEM_FUNCTION_EQ,
            engine: op::WAIT_REG_MEM_ENGINE_ME,
            addr: 0x1234,
            reference: 0xFF,
            mask: 0xFF,
            poll_interval: 0x20,
        });
        assert_eq!(
            builder.as_slice(),
            &[0xC005_3C00, 0x3, 0x1234, 0, 0xFF, 0xFF, 0x20]
        );
    }

    #[test]
    fn indirect_buffer_masks_address_and_packs_control() {
        let mut builder = PacketBuilder::new();
        builder.indirect_buffer_valid(0x1_0002_ABC4, 0x100, 3);
        let control = 0x100 | (3 << 24) | INDIRECT_BUFFER_VALID;
        assert_eq!(
            builder.as_slice(),
            &[0xC002_3F00, 0x0002_ABC4, 0x1, control]
        );
    }

    #[test]
    fn nop_packet_pads_with_zero_words() {
        let mut builder = PacketBuilder::new();
        builder.nop(2);
        assert_eq!(builder.as_slice(), &[0xC002_1000, 0, 0]);
    }

    #[test]
    fn event_write_encodes_type_and_index() {
        assert_eq!(event_word(7, 4), 0x407);
        let mut builder = PacketBuilder::new();
        builder.event_write(event_word(7, 4));
        assert_eq!(builder.as_slice(), &[0xC000_4600, 0x407]);
    }
}
