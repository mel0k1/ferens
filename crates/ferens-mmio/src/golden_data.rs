use crate::golden::GoldenEntry;

pub const GFX_POLARIS10: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0xcd4,
        and_mask: 0xc0fc0,
        or_mask: 0xc0200,
    },
    GoldenEntry {
        offset: 0x2684,
        and_mask: 0x1f3cf,
        or_mask: 0x7208,
    },
    GoldenEntry {
        offset: 0x2686,
        and_mask: 0xf000000,
        or_mask: 0xf000000,
    },
    GoldenEntry {
        offset: 0x2683,
        and_mask: 0x1ff,
        or_mask: 0x40,
    },
    GoldenEntry {
        offset: 0x260d,
        and_mask: 0xf00fffff,
        or_mask: 0x400,
    },
    GoldenEntry {
        offset: 0x22fc,
        and_mask: 0xffffffff,
        or_mask: 0x20000001,
    },
    GoldenEntry {
        offset: 0xc281,
        and_mask: 0xff0f,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0xa0d4,
        and_mask: 0x3f3fffff,
        or_mask: 0x16000012,
    },
    GoldenEntry {
        offset: 0xa0d5,
        and_mask: 0x3f,
        or_mask: 0x2a,
    },
    GoldenEntry {
        offset: 0xec49,
        and_mask: 0x3,
        or_mask: 0x1003c,
    },
    GoldenEntry {
        offset: 0xec9d,
        and_mask: 0xffffffff,
        or_mask: 0x1003c,
    },
    GoldenEntry {
        offset: 0x2300,
        and_mask: 0x7f80000,
        or_mask: 0x7180000,
    },
    GoldenEntry {
        offset: 0x2542,
        and_mask: 0xf000f,
        or_mask: 0xb0000,
    },
    GoldenEntry {
        offset: 0x2b80,
        and_mask: 0x100000,
        or_mask: 0xf31fff7f,
    },
    GoldenEntry {
        offset: 0x2b05,
        and_mask: 0x3ff,
        or_mask: 0xf7,
    },
    GoldenEntry {
        offset: 0x2b04,
        and_mask: 0xffffffff,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0x2232,
        and_mask: 0x4,
        or_mask: 0x4,
    },
];

pub const GFX_POLARIS10_COMMON: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0xc200,
        and_mask: 0xffffffff,
        or_mask: 0xe0000000,
    },
    GoldenEntry {
        offset: 0xa0d4,
        and_mask: 0xffffffff,
        or_mask: 0x16000012,
    },
    GoldenEntry {
        offset: 0xa0d5,
        and_mask: 0xffffffff,
        or_mask: 0x2a,
    },
    GoldenEntry {
        offset: 0x263e,
        and_mask: 0xffffffff,
        or_mask: 0x22011003,
    },
    GoldenEntry {
        offset: 0x31dc,
        and_mask: 0xffffffff,
        or_mask: 0x800,
    },
    GoldenEntry {
        offset: 0x31dd,
        and_mask: 0xffffffff,
        or_mask: 0x800,
    },
    GoldenEntry {
        offset: 0x31e6,
        and_mask: 0xffffffff,
        or_mask: 0xff7fbf,
    },
    GoldenEntry {
        offset: 0x31e7,
        and_mask: 0xffffffff,
        or_mask: 0xff7faf,
    },
];

pub const GFX_POLARIS11: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0x2684,
        and_mask: 0xf3cf,
        or_mask: 0x7208,
    },
    GoldenEntry {
        offset: 0x2686,
        and_mask: 0xf000000,
        or_mask: 0xf000000,
    },
    GoldenEntry {
        offset: 0x2683,
        and_mask: 0x1ff,
        or_mask: 0x40,
    },
    GoldenEntry {
        offset: 0x260d,
        and_mask: 0xf00fffff,
        or_mask: 0x400,
    },
    GoldenEntry {
        offset: 0x22fc,
        and_mask: 0xffffffff,
        or_mask: 0x20000001,
    },
    GoldenEntry {
        offset: 0xc281,
        and_mask: 0xff0f,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0xa0d4,
        and_mask: 0x3f3fffff,
        or_mask: 0x16000012,
    },
    GoldenEntry {
        offset: 0xa0d5,
        and_mask: 0x3f,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0xec49,
        and_mask: 0x3,
        or_mask: 0x1003c,
    },
    GoldenEntry {
        offset: 0xec9d,
        and_mask: 0xffffffff,
        or_mask: 0x1003c,
    },
    GoldenEntry {
        offset: 0x2300,
        and_mask: 0x7f80000,
        or_mask: 0x1180000,
    },
    GoldenEntry {
        offset: 0x2542,
        and_mask: 0xf000f,
        or_mask: 0xb0000,
    },
    GoldenEntry {
        offset: 0x2b80,
        and_mask: 0x100000,
        or_mask: 0xf31fff7f,
    },
    GoldenEntry {
        offset: 0x2b05,
        and_mask: 0x3ff,
        or_mask: 0xf3,
    },
    GoldenEntry {
        offset: 0x2b04,
        and_mask: 0xffffffff,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0x2b03,
        and_mask: 0xffffffff,
        or_mask: 0x3210,
    },
    GoldenEntry {
        offset: 0x2232,
        and_mask: 0x4,
        or_mask: 0x4,
    },
];

pub const GFX_POLARIS11_COMMON: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0xc200,
        and_mask: 0xffffffff,
        or_mask: 0xe0000000,
    },
    GoldenEntry {
        offset: 0x263e,
        and_mask: 0xffffffff,
        or_mask: 0x22011002,
    },
    GoldenEntry {
        offset: 0x31dc,
        and_mask: 0xffffffff,
        or_mask: 0x800,
    },
    GoldenEntry {
        offset: 0x31dd,
        and_mask: 0xffffffff,
        or_mask: 0x800,
    },
    GoldenEntry {
        offset: 0x31e6,
        and_mask: 0xffffffff,
        or_mask: 0xff7fbf,
    },
    GoldenEntry {
        offset: 0x31e7,
        and_mask: 0xffffffff,
        or_mask: 0xff7faf,
    },
];

pub const GMC_POLARIS10: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0x9e1,
        and_mask: 0x3,
        or_mask: 0x0,
    },
    GoldenEntry {
        offset: 0x52c,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52d,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52e,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52f,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
];

pub const GMC_POLARIS11: &[GoldenEntry] = &[
    GoldenEntry {
        offset: 0x52c,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52d,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52e,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
    GoldenEntry {
        offset: 0x52f,
        and_mask: 0xfffffff,
        or_mask: 0xfffffff,
    },
];
