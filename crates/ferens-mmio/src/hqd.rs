pub const SRBM_GFX_CNTL: u32 = 0x391;
pub const CP_MEC_CNTL: u32 = 0x208D;
pub const CP_MEC_ME1_UCODE_ADDR: u32 = 0xF81A;
pub const CP_MEC_ME1_UCODE_DATA: u32 = 0xF81B;
pub const CP_MEC_ME2_UCODE_ADDR: u32 = 0xF81C;
pub const CP_MEC_ME2_UCODE_DATA: u32 = 0xF81D;
pub const CP_MEC_DOORBELL_RANGE_LOWER: u32 = 0x305C;
pub const CP_MEC_DOORBELL_RANGE_UPPER: u32 = 0x305D;
pub const CP_PQ_WPTR_POLL_CNTL: u32 = 0x3083;
pub const CP_PQ_STATUS: u32 = 0x30B8;
pub const CP_MQD_BASE_ADDR: u32 = 0x3245;
pub const CP_MQD_BASE_ADDR_HI: u32 = 0x3246;
pub const CP_HQD_ACTIVE: u32 = 0x3247;
pub const CP_HQD_VMID: u32 = 0x3248;
pub const CP_HQD_PERSISTENT_STATE: u32 = 0x3249;
pub const CP_HQD_PIPE_PRIORITY: u32 = 0x324A;
pub const CP_HQD_QUEUE_PRIORITY: u32 = 0x324B;
pub const CP_HQD_QUANTUM: u32 = 0x324C;
pub const CP_HQD_PQ_BASE: u32 = 0x324D;
pub const CP_HQD_PQ_BASE_HI: u32 = 0x324E;
pub const CP_HQD_PQ_RPTR: u32 = 0x324F;
pub const CP_HQD_PQ_RPTR_REPORT_ADDR: u32 = 0x3250;
pub const CP_HQD_PQ_RPTR_REPORT_ADDR_HI: u32 = 0x3251;
pub const CP_HQD_PQ_WPTR_POLL_ADDR: u32 = 0x3252;
pub const CP_HQD_PQ_WPTR_POLL_ADDR_HI: u32 = 0x3253;
pub const CP_HQD_PQ_DOORBELL_CONTROL: u32 = 0x3254;
pub const CP_HQD_PQ_WPTR: u32 = 0x3255;
pub const CP_HQD_PQ_CONTROL: u32 = 0x3256;
pub const CP_HQD_IB_BASE_ADDR: u32 = 0x3257;
pub const CP_HQD_IB_BASE_ADDR_HI: u32 = 0x3258;
pub const CP_HQD_IB_RPTR: u32 = 0x3259;
pub const CP_HQD_IB_CONTROL: u32 = 0x325A;
pub const CP_HQD_IQ_TIMER: u32 = 0x325B;
pub const CP_HQD_IQ_RPTR: u32 = 0x325C;
pub const CP_HQD_DEQUEUE_REQUEST: u32 = 0x325D;
pub const CP_MQD_CONTROL: u32 = 0x3267;
pub const CP_HQD_CTX_SAVE_CONTROL: u32 = 0x3272;
pub const CP_HQD_ERROR: u32 = 0x3278;

pub const CP_MEC_CNTL_ME1_HALT: u32 = 1 << 30;
pub const CP_MEC_CNTL_ME2_HALT: u32 = 1 << 28;
pub const CP_ME1_PIPE0_INT_CNTL: u32 = 0x3085;
pub const CP_INT_TIME_STAMP_INT_ENABLE: u32 = 0x400_0000;
pub const CPC_INT_STATUS: u32 = 0x30B5;
pub const COMPUTE_EOP_TRIGGER: u32 = 0x2000_0000;
pub const CP_HQD_ACTIVE_BIT: u32 = 1;
pub const CP_HQD_DEQUEUE_DRAIN: u32 = 1;
pub const CP_HQD_DEQUEUE_RESET_WAVES: u32 = 2;
pub const CP_HQD_DEQUEUE_IQ_REQ_PEND: u32 = 1 << 4;
pub const CP_HQD_PERSISTENT_PRELOAD_SIZE: u32 = 0x53 << 8;
pub const CP_HQD_PERSISTENT_PRELOAD_SIZE_MASK: u32 = 0x3FF00;
pub const CP_HQD_QUANTUM_EN: u32 = 1;
pub const CP_HQD_QUANTUM_SCALE: u32 = 1 << 4;
pub const CP_HQD_QUANTUM_DURATION: u32 = 10 << 8;
pub const CP_HQD_PQ_CONTROL_QUEUE_SIZE_SHIFT: u32 = 0;
pub const CP_HQD_PQ_CONTROL_RPTR_BLOCK_SHIFT: u32 = 8;
pub const CP_HQD_PQ_CONTROL_RPTR_BLOCK_SIZE: u32 = 5;
pub const CP_HQD_PQ_CONTROL_MTYPE_MASK: u32 = 0x18000;
pub const CP_HQD_PQ_CONTROL_UNORD_DISPATCH: u32 = 1 << 28;
pub const CP_HQD_PQ_CONTROL_ROQ_PQ_IB_FLIP: u32 = 1 << 29;
pub const CP_HQD_PQ_CONTROL_PRIV_STATE: u32 = 1 << 30;
pub const CP_HQD_PQ_CONTROL_KMD_QUEUE: u32 = 1 << 31;
pub const CP_HQD_PQ_DOORBELL_OFFSET_SHIFT: u32 = 10;
pub const CP_HQD_PQ_DOORBELL_SOURCE: u32 = 1 << 28;
pub const CP_HQD_PQ_DOORBELL_EN: u32 = 1 << 30;
pub const CP_HQD_PQ_DOORBELL_HIT: u32 = 1 << 31;
pub const CP_HQD_IB_CONTROL_MIN_AVAIL: u32 = 3 << 20;
pub const CP_HQD_IB_CONTROL_MIN_AVAIL_MASK: u32 = 0x300000;
pub const CP_HQD_IB_CONTROL_MTYPE: u32 = 3 << 27;
pub const CP_HQD_IB_CONTROL_MTYPE_MASK: u32 = 0x18000000;
pub const CP_HQD_IQ_TIMER_MTYPE: u32 = 3 << 27;
pub const CP_HQD_IQ_TIMER_MTYPE_MASK: u32 = 0x18000000;
pub const CP_HQD_CTX_SAVE_MTYPE: u32 = 3 << 1;
pub const CP_HQD_CTX_SAVE_MTYPE_MASK: u32 = 0x6;
pub const CP_MQD_CONTROL_VMID_MASK: u32 = 0xF;
pub const CP_MEC_DOORBELL_RANGE_SHIFT: u32 = 2;
pub const CP_PQ_WPTR_POLL_CNTL_EN: u32 = 1 << 31;
pub const CP_PQ_STATUS_DOORBELL_ENABLE: u32 = 1 << 1;

pub const MEC_ME1: u32 = 1;
pub const PIPE_0: u32 = 0;
pub const QUEUE_0: u32 = 0;
pub const VMID_0: u32 = 0;

pub const DOORBELL_KIQ: u32 = 0x000;
pub const DOORBELL_MEC_RING0: u32 = 0x010;
pub const DOORBELL_MEC_RING7: u32 = 0x017;
pub const DOORBELL_INDEX: u32 = DOORBELL_MEC_RING0;

pub fn srbm_gfx_cntl(mec: u32, pipe: u32, queue: u32, vmid: u32) -> u32 {
    (pipe & 0x3) | ((mec & 0x3) << 2) | ((vmid & 0xF) << 4) | ((queue & 0x7) << 8)
}

pub fn persistent_state(current: u32) -> u32 {
    (current & !CP_HQD_PERSISTENT_PRELOAD_SIZE_MASK) | CP_HQD_PERSISTENT_PRELOAD_SIZE
}

pub fn quantum() -> u32 {
    CP_HQD_QUANTUM_EN | CP_HQD_QUANTUM_SCALE | CP_HQD_QUANTUM_DURATION
}

pub fn pq_control(ring_dwords: u32) -> u32 {
    ((ring_dwords.trailing_zeros() - 1) << CP_HQD_PQ_CONTROL_QUEUE_SIZE_SHIFT)
        | (CP_HQD_PQ_CONTROL_RPTR_BLOCK_SIZE << CP_HQD_PQ_CONTROL_RPTR_BLOCK_SHIFT)
        | CP_HQD_PQ_CONTROL_PRIV_STATE
        | CP_HQD_PQ_CONTROL_KMD_QUEUE
}

pub fn doorbell_control(index: u32) -> u32 {
    (index << CP_HQD_PQ_DOORBELL_OFFSET_SHIFT) | CP_HQD_PQ_DOORBELL_EN
}

pub fn doorbell_range_lower(index: u32) -> u32 {
    index << CP_MEC_DOORBELL_RANGE_SHIFT
}

pub fn doorbell_range_upper(index: u32) -> u32 {
    index << CP_MEC_DOORBELL_RANGE_SHIFT
}

pub fn ib_control(current: u32) -> u32 {
    (current & !(CP_HQD_IB_CONTROL_MIN_AVAIL_MASK | CP_HQD_IB_CONTROL_MTYPE_MASK))
        | CP_HQD_IB_CONTROL_MIN_AVAIL
        | CP_HQD_IB_CONTROL_MTYPE
}

pub fn iq_timer(current: u32) -> u32 {
    (current & !CP_HQD_IQ_TIMER_MTYPE_MASK) | CP_HQD_IQ_TIMER_MTYPE
}

pub fn ctx_save_control(current: u32) -> u32 {
    (current & !CP_HQD_CTX_SAVE_MTYPE_MASK) | CP_HQD_CTX_SAVE_MTYPE
}

pub fn mqd_control(current: u32) -> u32 {
    current & !CP_MQD_CONTROL_VMID_MASK
}

pub fn pq_base_word(va: u64) -> u32 {
    (va >> 8) as u32
}

pub fn pq_base_hi_word(va: u64) -> u32 {
    (va >> 40) as u32
}

pub fn byte_addr_word(addr: u64) -> u32 {
    (addr & !0x3) as u32
}

pub fn byte_addr_hi_word(addr: u64) -> u32 {
    ((addr & !0x3) >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srbm_selects_mec1_pipe0_queue0() {
        assert_eq!(srbm_gfx_cntl(MEC_ME1, PIPE_0, QUEUE_0, VMID_0), 0x4);
        assert_eq!(
            srbm_gfx_cntl(2, 1, 3, 5),
            (1 << 0) | (2 << 2) | (5 << 4) | (3 << 8)
        );
        assert_eq!(srbm_gfx_cntl(0, 0, 0, 0), 0);
    }

    #[test]
    fn pq_control_matches_kernel_encoding() {
        assert_eq!(pq_control(1024), 9 | (5 << 8) | (1 << 30) | (1 << 31));
        assert_eq!(pq_control(256), 7 | (5 << 8) | (1 << 30) | (1 << 31));
    }

    #[test]
    fn doorbell_control_encodes_index_and_enable() {
        assert_eq!(
            doorbell_control(DOORBELL_MEC_RING0),
            (0x10 << 10) | (1 << 30)
        );
        assert_eq!(doorbell_control(0), 1 << 30);
    }

    #[test]
    fn doorbell_range_uses_byte_offsets() {
        assert_eq!(doorbell_range_lower(0), 0);
        assert_eq!(doorbell_range_upper(DOORBELL_MEC_RING7), 0x17 << 2);
    }

    #[test]
    fn persistent_state_sets_preload_only() {
        assert_eq!(persistent_state(0), 0x5300);
        assert_eq!(persistent_state(0x1234_5678), 0x1234_5378);
        assert_eq!(persistent_state(0x4000_4000), 0x4000_5300);
    }

    #[test]
    fn quantum_matches_kernel_values() {
        assert_eq!(quantum(), 1 | (1 << 4) | (10 << 8));
    }

    #[test]
    fn rmw_helpers_keep_other_fields() {
        assert_eq!(ib_control(0xFFFF_FFFF), 0xFFFF_FFFF);
        assert_eq!(ib_control(0), (3 << 20) | (3 << 27));
        assert_eq!(iq_timer(0x0000_00FF), 0x1800_00FF);
        assert_eq!(ctx_save_control(0x1), 0x7);
        assert_eq!(mqd_control(0x0000_0005), 0);
        assert_eq!(mqd_control(0xFFFF_FFFF), 0xFFFF_FFF0);
    }

    #[test]
    fn pq_base_and_byte_addresses_match_units() {
        assert_eq!(pq_base_word(0x0400_0000), 0x0004_0000);
        assert_eq!(pq_base_hi_word(0x0000_0400_0000), 0);
        assert_eq!(byte_addr_word(0x0400_A006), 0x0400_A004);
        assert_eq!(byte_addr_hi_word(0x0000_0400_A000), 0);
    }
}
