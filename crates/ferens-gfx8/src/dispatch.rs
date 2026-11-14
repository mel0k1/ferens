use crate::isa;
use crate::op;
use crate::pm4::PacketBuilder;
use crate::regs;

pub const KERNARG_OFFSET: usize = 0;
pub const FENCE_OFFSET: usize = 256;
pub const OUTPUT_OFFSET: usize = 512;
pub const FENCE_MAGIC: u32 = 0x5A5A_0002;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchPlan {
    pub kernel_va: u64,
    pub kernarg_va: u64,
    pub fence_va: u64,
    pub output_va: u64,
    pub grid: [u32; 3],
    pub group: [u32; 3],
}

pub fn push(builder: &mut PacketBuilder, plan: &DispatchPlan) {
    let rsrc1 = isa::pgm_rsrc1(isa::VGPRS, isa::SGPRS);
    let rsrc2 = isa::pgm_rsrc2(isa::USER_SGPRS);
    builder.set_sh_reg(
        regs::sh_offset(regs::COMPUTE_PGM_LO),
        &[
            (plan.kernel_va >> 8) as u32,
            ((plan.kernel_va >> 40) & 0xFF) as u32,
        ],
    );
    builder.set_sh_reg(regs::sh_offset(regs::COMPUTE_PGM_RSRC1), &[rsrc1]);
    builder.set_sh_reg(regs::sh_offset(regs::COMPUTE_PGM_RSRC2), &[rsrc2]);
    builder.set_sh_reg(
        regs::sh_offset(regs::COMPUTE_NUM_THREAD_X),
        &[plan.group[0], plan.group[1], plan.group[2]],
    );
    builder.set_sh_reg(
        regs::sh_offset(regs::COMPUTE_USER_DATA_0),
        &[
            plan.kernarg_va as u32,
            ((plan.kernarg_va >> 32) & 0xFFFF) as u32,
        ],
    );
    builder.dispatch_direct(
        plan.grid[0],
        plan.grid[1],
        plan.grid[2],
        regs::DISPATCH_SHADER_EN,
    );
    builder.write_data(
        op::WRITE_DATA_DST_SEL_MEMORY,
        op::WRITE_DATA_ENGINE_ME,
        true,
        plan.fence_va,
        &[FENCE_MAGIC],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PLAN: DispatchPlan = DispatchPlan {
        kernel_va: 0x0000_0001_0000_0000,
        kernarg_va: 0x0000_0001_0001_0000,
        fence_va: 0x0000_0001_0001_0100,
        output_va: 0x0000_0001_0001_0200,
        grid: [1, 1, 1],
        group: [64, 1, 1],
    };

    #[test]
    fn plan_matches_kernel_convention_word_by_word() {
        let mut builder = PacketBuilder::new();
        push(&mut builder, &SAMPLE_PLAN);
        let expected: [u32; 29] = [
            0xC002_7600,
            0x20C,
            0x0100_0000,
            0x0000_0000,
            0xC001_7600,
            0x212,
            isa::pgm_rsrc1(isa::VGPRS, isa::SGPRS),
            0xC001_7600,
            0x213,
            isa::pgm_rsrc2(isa::USER_SGPRS),
            0xC003_7600,
            0x207,
            64,
            1,
            1,
            0xC002_7600,
            0x240,
            0x0001_0000,
            0x0000_0001,
            0xC003_1500,
            1,
            1,
            1,
            regs::DISPATCH_SHADER_EN,
            0xC003_3700,
            (op::WRITE_DATA_DST_SEL_MEMORY << 8)
                | (op::WRITE_DATA_ENGINE_ME << 30)
                | op::WRITE_DATA_WR_CONFIRM,
            0x0001_0100,
            0x0000_0001,
            FENCE_MAGIC,
        ];
        assert_eq!(builder.as_slice(), &expected);
    }

    #[test]
    fn sample_plan_offsets_match_layout_constants() {
        assert_eq!(
            SAMPLE_PLAN.fence_va,
            SAMPLE_PLAN.kernarg_va + FENCE_OFFSET as u64
        );
        assert_eq!(
            SAMPLE_PLAN.output_va,
            SAMPLE_PLAN.kernarg_va + OUTPUT_OFFSET as u64
        );
    }
}
