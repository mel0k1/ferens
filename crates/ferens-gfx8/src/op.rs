pub const NOP: u8 = 0x10;
pub const DISPATCH_DIRECT: u8 = 0x15;
pub const DISPATCH_INDIRECT: u8 = 0x16;
pub const INDIRECT_BUFFER_CONST: u8 = 0x33;
pub const WRITE_DATA: u8 = 0x37;
pub const WAIT_REG_MEM: u8 = 0x3C;
pub const INDIRECT_BUFFER: u8 = 0x3F;
pub const EVENT_WRITE: u8 = 0x46;
pub const SET_CONTEXT_REG: u8 = 0x69;
pub const SET_SH_REG: u8 = 0x76;

pub const WAIT_REG_MEM_FUNCTION_ALWAYS: u32 = 0;
pub const WAIT_REG_MEM_FUNCTION_EQ: u32 = 3;
pub const WAIT_REG_MEM_ENGINE_ME: u32 = 0;

pub const WRITE_DATA_DST_SEL_REGISTER: u32 = 0;
pub const WRITE_DATA_DST_SEL_MEMORY: u32 = 5;
pub const WRITE_DATA_ENGINE_ME: u32 = 0;
pub const WRITE_DATA_WR_CONFIRM: u32 = 1 << 20;

pub const EVENT_TYPE_SHIFT: u32 = 0;
pub const EVENT_INDEX_SHIFT: u32 = 8;
