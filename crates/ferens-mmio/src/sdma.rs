use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{BarOps, Chip, MappedBar, Mmio, PciDevice};
use ferens_gfx8::ring::write_packet;

use crate::fw::{self, UcodeFirmware};
use crate::gtt::HostBuffer;
use crate::hdp;
use crate::hqd;
use crate::iommu;
use crate::queue::{
    program_vm0, taken_by_driver, GttBuffer, GTT_BASE_VA, GTT_WINDOW_BYTES, PTE_TABLE_OFFSET,
    SDMA_INSTANCE_STRIDE, SDMA_LAYOUT_END, SDMA_META_INSTANCE_STRIDE, SDMA_META_OFFSET,
    SDMA_RING_BYTES, SDMA_RING_OFFSET, SDMA_SCRATCH_OFFSET, VRAM_BASE,
};
use crate::vm::{self, GPU_PAGE_BYTES};

pub const SDMA_REGISTER_STRIDE: u32 = 0x200;
pub const UCODE_ADDR: u32 = 0x3400;
pub const UCODE_DATA: u32 = 0x3401;
pub const CNTL: u32 = 0x3404;
pub const TILING_CONFIG: u32 = 0x3406;
pub const SEM_WAIT_FAIL_TIMER_CNTL: u32 = 0x3409;
pub const STATUS_REG: u32 = 0x340d;
pub const F32_CNTL: u32 = 0x3412;
pub const GFX_RB_CNTL: u32 = 0x3480;
pub const GFX_RB_BASE: u32 = 0x3481;
pub const GFX_RB_BASE_HI: u32 = 0x3482;
pub const GFX_RB_RPTR: u32 = 0x3483;
pub const GFX_RB_WPTR: u32 = 0x3484;
pub const GFX_RB_WPTR_POLL_CNTL: u32 = 0x3485;
pub const GFX_RB_RPTR_ADDR_HI: u32 = 0x3488;
pub const GFX_RB_RPTR_ADDR_LO: u32 = 0x3489;
pub const GFX_IB_CNTL: u32 = 0x348a;
pub const GFX_IB_RPTR: u32 = 0x348b;
pub const GFX_IB_OFFSET: u32 = 0x348c;
pub const GFX_DOORBELL: u32 = 0x3492;
pub const GFX_VIRTUAL_ADDR: u32 = 0x34a7;
pub const GFX_APE1_CNTL: u32 = 0x34a8;
pub const GB_ADDR_CONFIG: u32 = 0x263e;

pub const F32_HALT: u32 = 0x1;
pub const CNTL_ATC_L1_ENABLE: u32 = 0x2;
pub const CNTL_AUTO_CTXSW_ENABLE: u32 = 0x4_0000;
pub const STATUS_IDLE: u32 = 0x1;
pub const STATUS_REG_IDLE: u32 = 0x2;
pub const STATUS_RB_EMPTY: u32 = 0x4;
pub const RB_ENABLE: u32 = 0x1;
pub const RB_SIZE_MASK: u32 = 0x3e;
pub const RB_SIZE_SHIFT: u32 = 1;
pub const RB_SWAP_ENABLE: u32 = 0x200;
pub const RPTR_WRITEBACK_ENABLE: u32 = 0x1000;
pub const RPTR_WRITEBACK_SWAP_ENABLE: u32 = 0x2000;
pub const POLL_ENABLE: u32 = 0x1;
pub const IB_ENABLE: u32 = 0x1;
pub const DOORBELL_ENABLE: u32 = 0x1000_0000;
pub const RB_RPTR_ADDR_LO_MASK: u32 = 0xffff_fffc;

pub const OP_NOP: u32 = 0;
pub const OP_COPY: u32 = 1;
pub const OP_FENCE: u32 = 5;
pub const SUBOP_COPY_LINEAR: u32 = 0;
pub const COPY_COUNT_MAX: usize = 0x3f_ffff;

pub const FENCE_MAGIC: u32 = 0x5344_4d41;
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(5);
pub const INSTANCES: usize = 2;

pub const SDMA_GTT_CURSOR_START: u32 = (GTT_WINDOW_BYTES / 2 / GPU_PAGE_BYTES) as u32;

pub fn scratch_va() -> u64 {
    SDMA_SCRATCH_OFFSET
}

pub fn meta_va(instance: usize) -> u64 {
    SDMA_META_OFFSET + instance as u64 * SDMA_META_INSTANCE_STRIDE
}

pub fn ring_va(instance: usize) -> u64 {
    SDMA_RING_OFFSET + instance as u64 * SDMA_INSTANCE_STRIDE
}

pub fn copy_packet(dst_va: u64, src_va: u64, bytes: usize) -> Vec<u32> {
    vec![
        OP_COPY | (SUBOP_COPY_LINEAR << 8),
        bytes as u32,
        0,
        src_va as u32,
        (src_va >> 32) as u32,
        dst_va as u32,
        (dst_va >> 32) as u32,
    ]
}

pub fn fence_packet(fence_va: u64) -> Vec<u32> {
    vec![
        OP_NOP,
        OP_FENCE,
        fence_va as u32,
        (fence_va >> 32) as u32,
        FENCE_MAGIC,
    ]
}

pub struct SdmaStatus {
    pub instance: usize,
    pub status: u32,
    pub rptr_dw: u32,
    pub wptr_dw: u32,
}

impl SdmaStatus {
    pub fn report(&self) -> String {
        format!(
            "sdma{} status {:#010x} idle {} reg_idle {} rb_empty {} rptr {} dwords wptr {} dwords",
            self.instance,
            self.status,
            self.status & STATUS_IDLE != 0,
            self.status & STATUS_REG_IDLE != 0,
            self.status & STATUS_RB_EMPTY != 0,
            self.rptr_dw,
            self.wptr_dw
        )
    }
}

pub fn engine_status<B: BarOps>(regs: &Mmio<B>, instance: usize) -> SdmaStatus {
    let offset = instance as u32 * SDMA_REGISTER_STRIDE;
    SdmaStatus {
        instance,
        status: regs.read_reg_direct(STATUS_REG + offset),
        rptr_dw: regs.read_reg_direct(GFX_RB_RPTR + offset) >> 2,
        wptr_dw: regs.read_reg_direct(GFX_RB_WPTR + offset) >> 2,
    }
}

pub fn load_engine_ucode<B: BarOps>(
    regs: &Mmio<B>,
    instance: usize,
    firmware: &UcodeFirmware,
) -> io::Result<()> {
    let offset = instance as u32 * SDMA_REGISTER_STRIDE;
    regs.write_reg_direct(F32_CNTL + offset, F32_HALT);
    regs.write_reg_direct(UCODE_ADDR + offset, 0);
    for word in &firmware.words {
        regs.write_reg_direct(UCODE_DATA + offset, *word);
    }
    verify_engine_ucode(regs, instance, firmware)
}

pub fn verify_engine_ucode<B: BarOps>(
    regs: &Mmio<B>,
    instance: usize,
    firmware: &UcodeFirmware,
) -> io::Result<()> {
    let offset = instance as u32 * SDMA_REGISTER_STRIDE;
    let mut readback = Vec::with_capacity(firmware.words.len());
    for index in 0..firmware.words.len() {
        regs.write_reg_direct(UCODE_ADDR + offset, index as u32);
        readback.push(regs.read_reg_direct(UCODE_DATA + offset));
    }
    let expected = fw::crc32(&firmware.words);
    let actual = fw::crc32(&readback);
    if actual != expected {
        return Err(io::Error::other(format!(
            "sdma{instance} ucode readback crc mismatch: want {expected:#010x}, got {actual:#010x}"
        )));
    }
    regs.write_reg_direct(UCODE_ADDR + offset, firmware.version);
    Ok(())
}

pub fn resume_ring<B: BarOps>(regs: &Mmio<B>, instance: usize, ring_va: u64, wb_va: u64) {
    let offset = instance as u32 * SDMA_REGISTER_STRIDE;
    let ring_dw: usize = SDMA_RING_BYTES / 4;
    regs.write_reg_direct(GFX_VIRTUAL_ADDR + offset, 0);
    regs.write_reg_direct(GFX_APE1_CNTL + offset, 0);
    regs.write_reg_direct(
        TILING_CONFIG + offset,
        regs.read_reg_direct(GB_ADDR_CONFIG) & 0x70,
    );
    regs.write_reg_direct(SEM_WAIT_FAIL_TIMER_CNTL + offset, 0);
    let rb_cntl = (regs.read_reg_direct(GFX_RB_CNTL + offset)
        & !(RB_ENABLE | RB_SWAP_ENABLE | RPTR_WRITEBACK_ENABLE | RPTR_WRITEBACK_SWAP_ENABLE))
        | (ring_dw.trailing_zeros() << RB_SIZE_SHIFT);
    regs.write_reg_direct(GFX_RB_CNTL + offset, rb_cntl);
    regs.write_reg_direct(GFX_RB_RPTR + offset, 0);
    regs.write_reg_direct(GFX_RB_WPTR + offset, 0);
    regs.write_reg_direct(GFX_IB_RPTR + offset, 0);
    regs.write_reg_direct(GFX_IB_OFFSET + offset, 0);
    regs.write_reg_direct(GFX_RB_RPTR_ADDR_HI + offset, (wb_va >> 32) as u32);
    regs.write_reg_direct(
        GFX_RB_RPTR_ADDR_LO + offset,
        wb_va as u32 & RB_RPTR_ADDR_LO_MASK,
    );
    regs.write_reg_direct(GFX_RB_CNTL + offset, rb_cntl | RPTR_WRITEBACK_ENABLE);
    regs.write_reg_direct(GFX_RB_BASE + offset, (ring_va >> 8) as u32);
    regs.write_reg_direct(GFX_RB_BASE_HI + offset, (ring_va >> 40) as u32);
    let doorbell = regs.read_reg_direct(GFX_DOORBELL + offset);
    regs.write_reg_direct(GFX_DOORBELL + offset, doorbell & !DOORBELL_ENABLE);
    let poll = regs.read_reg_direct(GFX_RB_WPTR_POLL_CNTL + offset);
    regs.write_reg_direct(GFX_RB_WPTR_POLL_CNTL + offset, poll & !POLL_ENABLE);
    regs.write_reg_direct(
        GFX_RB_CNTL + offset,
        rb_cntl | RPTR_WRITEBACK_ENABLE | RB_ENABLE,
    );
    let ib_cntl = regs.read_reg_direct(GFX_IB_CNTL + offset);
    regs.write_reg_direct(GFX_IB_CNTL + offset, ib_cntl | IB_ENABLE);
    let f32_cntl = regs.read_reg_direct(F32_CNTL + offset);
    regs.write_reg_direct(F32_CNTL + offset, f32_cntl & !F32_HALT);
    let cntl = regs.read_reg_direct(CNTL + offset);
    regs.write_reg_direct(
        CNTL + offset,
        cntl | CNTL_AUTO_CTXSW_ENABLE | CNTL_ATC_L1_ENABLE,
    );
}

pub fn stop_engine<B: BarOps>(regs: &Mmio<B>, instance: usize) {
    let offset = instance as u32 * SDMA_REGISTER_STRIDE;
    let rb_cntl = regs.read_reg_direct(GFX_RB_CNTL + offset);
    regs.write_reg_direct(GFX_RB_CNTL + offset, rb_cntl & !RB_ENABLE);
    let ib_cntl = regs.read_reg_direct(GFX_IB_CNTL + offset);
    regs.write_reg_direct(GFX_IB_CNTL + offset, ib_cntl & !IB_ENABLE);
    let f32_cntl = regs.read_reg_direct(F32_CNTL + offset);
    regs.write_reg_direct(F32_CNTL + offset, f32_cntl | F32_HALT);
}

pub struct Sdma {
    regs: Mmio<MappedBar>,
    aperture: MappedBar,
    pub fw_versions: [u32; INSTANCES],
    ring_dw: usize,
    wp: usize,
    gtt_cursor: u32,
    destroyed: bool,
}

impl Sdma {
    pub fn open(slot_dir: &Path, chip: Chip) -> io::Result<Self> {
        taken_by_driver(slot_dir)?;
        let slot = slot_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        iommu::guard(Path::new("/"), &slot)?;
        let device = PciDevice::from_sysfs(slot_dir)?;
        let regs_info = device
            .regs_bar()
            .ok_or_else(|| io::Error::other("no register bar"))?;
        let aperture_info = device
            .aperture_bar()
            .ok_or_else(|| io::Error::other("no frame buffer aperture bar"))?;
        if aperture_info.size < SDMA_LAYOUT_END {
            return Err(io::Error::other(format!(
                "aperture bar is {:#x} bytes, need at least {:#x}",
                aperture_info.size, SDMA_LAYOUT_END
            )));
        }
        let regs_bar = MappedBar::from_sysfs(slot_dir, regs_info.index)?;
        let aperture = MappedBar::from_sysfs(slot_dir, aperture_info.index)?;
        let regs = Mmio::new(regs_bar);
        let firmware = fw::load_sdma(chip)?;
        let mut aperture = aperture;
        program_vm0(&regs, &mut aperture)?;
        let mut sdma = Self {
            regs,
            aperture,
            fw_versions: [firmware[0].version, firmware[1].version],
            ring_dw: SDMA_RING_BYTES / 4,
            wp: 0,
            gtt_cursor: SDMA_GTT_CURSOR_START,
            destroyed: false,
        };
        sdma.bring_up(&firmware)?;
        Ok(sdma)
    }

    fn bring_up(&mut self, firmware: &[UcodeFirmware; INSTANCES]) -> io::Result<()> {
        for instance in 0..INSTANCES {
            let offset = instance as u32 * SDMA_REGISTER_STRIDE;
            let f32_cntl = self.regs.read_reg_direct(F32_CNTL + offset);
            self.regs
                .write_reg_direct(F32_CNTL + offset, f32_cntl | F32_HALT);
        }
        for (instance, ucode) in firmware.iter().enumerate() {
            load_engine_ucode(&self.regs, instance, ucode)?;
        }
        for instance in 0..INSTANCES {
            self.zero_engine_memory(instance);
            resume_ring(
                &self.regs,
                instance,
                ring_va(instance),
                meta_va(instance) + 4,
            );
        }
        Ok(())
    }

    fn zero_engine_memory(&mut self, instance: usize) {
        let aperture = self.aperture.words_mut();
        let ring_first = ((ring_va(instance) - VRAM_BASE) / 4) as usize;
        let ring_last = ring_first + SDMA_RING_BYTES / 4;
        let meta_first = ((meta_va(instance) - VRAM_BASE) / 4) as usize;
        let meta_last = meta_first + (SDMA_META_INSTANCE_STRIDE / 4) as usize;
        for word in &mut aperture[ring_first..ring_last] {
            *word = 0;
        }
        for word in &mut aperture[meta_first..meta_last] {
            *word = 0;
        }
    }

    pub fn alloc_gtt(&mut self, pages: usize) -> io::Result<GttBuffer> {
        let window_pages = (GTT_WINDOW_BYTES / GPU_PAGE_BYTES) as usize;
        let cursor = self.gtt_cursor as usize;
        if cursor + pages > window_pages {
            return Err(io::Error::other("gtt window is exhausted"));
        }
        let host = HostBuffer::lock(pages)?;
        let first_page_index = (GTT_BASE_VA / GPU_PAGE_BYTES) as u32 + self.gtt_cursor;
        vm::fill_system_entries(
            &mut self.aperture,
            (PTE_TABLE_OFFSET - VRAM_BASE) as usize,
            first_page_index,
            host.pfns(),
        )?;
        self.gtt_cursor += pages as u32;
        hdp::invalidate_with_flush(&self.regs, hqd::VMID_0);
        Ok(GttBuffer {
            host,
            va: first_page_index as u64 * GPU_PAGE_BYTES,
        })
    }

    pub fn copy(&mut self, dst_va: u64, src_va: u64, bytes: usize) -> io::Result<()> {
        if bytes == 0 || bytes > COPY_COUNT_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "copy size must be 1..0x3fffff bytes",
            ));
        }
        let instance = 0;
        let fence_va = meta_va(instance);
        let mut packets = copy_packet(dst_va, src_va, bytes);
        packets.extend(fence_packet(fence_va));
        self.aperture.write_u32((fence_va - VRAM_BASE) as usize, 0);
        {
            let aperture = self.aperture.words_mut();
            let first = ((ring_va(instance) - VRAM_BASE) / 4) as usize;
            let ring = &mut aperture[first..first + self.ring_dw];
            let next = write_packet(ring, self.wp, &packets).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "packets do not fit the sdma ring",
                )
            })?;
            self.wp = next;
        }
        hdp::flush(&self.regs);
        self.regs
            .write_reg_direct(GFX_RB_WPTR, (self.wp as u32) << 2);
        self.wait_fence(instance)?;
        Ok(())
    }

    pub fn wait_fence(&self, instance: usize) -> io::Result<u32> {
        let started = Instant::now();
        let fence_offset = (meta_va(instance) - VRAM_BASE) as usize;
        loop {
            let fence = self.aperture.read_u32(fence_offset);
            if fence == FENCE_MAGIC {
                return Ok(fence);
            }
            if started.elapsed() > FENCE_TIMEOUT {
                let status = engine_status(&self.regs, instance);
                return Err(io::Error::other(format!(
                    "sdma{instance} fence timeout after {}ms\n{}",
                    started.elapsed().as_millis(),
                    status.report()
                )));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn status(&self, instance: usize) -> SdmaStatus {
        engine_status(&self.regs, instance)
    }

    pub fn read_pointer_dw(&self, instance: usize) -> u32 {
        self.aperture
            .read_u32((meta_va(instance) - VRAM_BASE) as usize + 4)
            >> 2
    }

    pub fn destroy(&mut self) -> io::Result<()> {
        for instance in 0..INSTANCES {
            stop_engine(&self.regs, instance);
        }
        self.destroyed = true;
        Ok(())
    }
}

impl Drop for Sdma {
    fn drop(&mut self) {
        if !self.destroyed {
            let _ = self.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeBar {
        cells: RefCell<Vec<u32>>,
        writes: RefCell<Vec<(usize, u32)>>,
        sram: RefCell<Vec<u32>>,
        cursor: Cell<usize>,
        addr_cell: usize,
        data_cell: usize,
    }

    impl FakeBar {
        fn for_instance(instance: usize) -> Self {
            Self {
                cells: RefCell::new(vec![0; BAR_WORDS]),
                writes: RefCell::new(Vec::new()),
                sram: RefCell::new(vec![0; BAR_WORDS]),
                cursor: Cell::new(0),
                addr_cell: (UCODE_ADDR + instance as u32 * SDMA_REGISTER_STRIDE) as usize * 4,
                data_cell: (UCODE_DATA + instance as u32 * SDMA_REGISTER_STRIDE) as usize * 4,
            }
        }
    }

    impl BarOps for FakeBar {
        fn read_u32(&self, offset: usize) -> u32 {
            if offset == self.data_cell {
                return self.sram.borrow()[self.cursor.get()];
            }
            self.cells.borrow()[offset / 4]
        }

        fn write_u32(&self, offset: usize, value: u32) {
            self.writes.borrow_mut().push((offset, value));
            if offset == self.addr_cell {
                self.cursor.set(value as usize);
            } else if offset == self.data_cell {
                let cursor = self.cursor.get();
                self.sram.borrow_mut()[cursor] = value;
                self.cursor.set(cursor + 1);
            }
            self.cells.borrow_mut()[offset / 4] = value;
        }

        fn size(&self) -> usize {
            0x40000
        }
    }

    const BAR_WORDS: usize = 0x10000;

    fn ucode() -> UcodeFirmware {
        UcodeFirmware {
            version: 0x2a,
            feature_version: 1,
            words: vec![0x1111_1111, 0x2222_2222, 0x3333_3333],
        }
    }

    #[test]
    fn copy_packet_matches_kernel_emit_copy() {
        let packet = copy_packet(0x5000_1000, 0x5000_0000, 4096);
        assert_eq!(packet, vec![1, 4096, 0, 0x5000_0000, 0, 0x5000_1000, 0]);
    }

    #[test]
    fn fence_packet_is_nop_then_fence_with_magic() {
        let packet = fence_packet(0x0401_9000);
        assert_eq!(packet, vec![OP_NOP, OP_FENCE, 0x0401_9000, 0, FENCE_MAGIC]);
    }

    #[test]
    fn ucode_streams_from_addr_zero_and_verifies_crc() {
        let mmio = Mmio::new(FakeBar::for_instance(0));
        load_engine_ucode(&mmio, 0, &ucode()).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(writes[0], (F32_CNTL as usize * 4, F32_HALT));
        assert_eq!(writes[1], (UCODE_ADDR as usize * 4, 0));
        assert_eq!(writes[2], (UCODE_DATA as usize * 4, 0x1111_1111));
        assert_eq!(writes[3], (UCODE_DATA as usize * 4, 0x2222_2222));
        assert_eq!(writes[4], (UCODE_DATA as usize * 4, 0x3333_3333));
        assert_eq!(writes[5], (UCODE_ADDR as usize * 4, 0));
        assert_eq!(writes[6], (UCODE_ADDR as usize * 4, 1));
        assert_eq!(writes[7], (UCODE_ADDR as usize * 4, 2));
        assert_eq!(writes.last().unwrap(), &(UCODE_ADDR as usize * 4, 0x2a));
    }

    #[test]
    fn ucode_instance_one_uses_strided_window() {
        let mmio = Mmio::new(FakeBar::for_instance(1));
        load_engine_ucode(&mmio, 1, &ucode()).unwrap();
        let writes = mmio.bar().writes.borrow();
        assert_eq!(
            writes[0],
            ((F32_CNTL + SDMA_REGISTER_STRIDE) as usize * 4, F32_HALT)
        );
        assert_eq!(
            writes[1],
            ((UCODE_ADDR + SDMA_REGISTER_STRIDE) as usize * 4, 0)
        );
        assert_eq!(
            writes[2],
            (
                (UCODE_DATA + SDMA_REGISTER_STRIDE) as usize * 4,
                0x1111_1111
            )
        );
    }

    #[test]
    fn corrupted_sram_fails_with_crc_mismatch() {
        let mmio = Mmio::new(FakeBar::for_instance(0));
        load_engine_ucode(&mmio, 0, &ucode()).unwrap();
        mmio.bar().sram.borrow_mut()[1] = 0xdead_beef;
        let error = verify_engine_ucode(&mmio, 0, &ucode()).unwrap_err();
        assert!(error
            .to_string()
            .contains("sdma0 ucode readback crc mismatch"));
    }

    #[test]
    fn resume_ring_enables_ring_with_writeback_and_kicks_without_doorbell() {
        let mmio = Mmio::new(FakeBar::for_instance(0));
        mmio.bar().cells.borrow_mut()[GB_ADDR_CONFIG as usize] = 0x2201_1003;
        resume_ring(&mmio, 0, 0x0401_7000, 0x0401_9004);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[GFX_VIRTUAL_ADDR as usize], 0);
        assert_eq!(cells[GFX_APE1_CNTL as usize], 0);
        assert_eq!(cells[TILING_CONFIG as usize], 0x2201_1003 & 0x70);
        assert_eq!(cells[SEM_WAIT_FAIL_TIMER_CNTL as usize], 0);
        let ring_dw = SDMA_RING_BYTES as u32 / 4;
        let expected_size = ring_dw.trailing_zeros() << RB_SIZE_SHIFT;
        assert_eq!(
            cells[GFX_RB_CNTL as usize],
            expected_size | RPTR_WRITEBACK_ENABLE | RB_ENABLE
        );
        assert_eq!(cells[GFX_RB_BASE as usize], 0x0401_7000 >> 8);
        assert_eq!(cells[GFX_RB_BASE_HI as usize], 0);
        assert_eq!(
            cells[GFX_RB_RPTR_ADDR_LO as usize],
            0x0401_9004 & RB_RPTR_ADDR_LO_MASK
        );
        assert_eq!(cells[GFX_RB_RPTR_ADDR_HI as usize], 0);
        assert_eq!(cells[GFX_DOORBELL as usize] & DOORBELL_ENABLE, 0);
        assert_eq!(cells[GFX_RB_WPTR_POLL_CNTL as usize] & POLL_ENABLE, 0);
        assert_eq!(cells[GFX_IB_CNTL as usize] & IB_ENABLE, IB_ENABLE);
        assert_eq!(cells[F32_CNTL as usize] & F32_HALT, 0);
        assert_eq!(
            cells[CNTL as usize] & (CNTL_AUTO_CTXSW_ENABLE | CNTL_ATC_L1_ENABLE),
            CNTL_AUTO_CTXSW_ENABLE | CNTL_ATC_L1_ENABLE
        );
    }

    #[test]
    fn stop_engine_disables_ring_and_halt() {
        let mmio = Mmio::new(FakeBar::for_instance(0));
        resume_ring(&mmio, 0, 0x0401_7000, 0x0401_9004);
        stop_engine(&mmio, 0);
        let cells = mmio.bar().cells.borrow();
        assert_eq!(cells[GFX_RB_CNTL as usize] & RB_ENABLE, 0);
        assert_eq!(cells[GFX_IB_CNTL as usize] & IB_ENABLE, 0);
        assert_eq!(cells[F32_CNTL as usize] & F32_HALT, F32_HALT);
    }

    #[test]
    fn status_reports_pointers_in_dwords() {
        let mmio = Mmio::new(FakeBar::for_instance(0));
        mmio.bar().cells.borrow_mut()[STATUS_REG as usize] = STATUS_IDLE | STATUS_RB_EMPTY;
        mmio.bar().cells.borrow_mut()[GFX_RB_RPTR as usize] = 0x40;
        mmio.bar().cells.borrow_mut()[GFX_RB_WPTR as usize] = 0x80;
        let status = engine_status(&mmio, 0);
        assert_eq!(status.rptr_dw, 0x10);
        assert_eq!(status.wptr_dw, 0x20);
        let report = status.report();
        assert!(report.contains("idle true"));
        assert!(report.contains("rb_empty true"));
    }

    #[test]
    fn gtt_cursor_starts_in_second_half_of_window() {
        assert_eq!(SDMA_GTT_CURSOR_START, 2048);
    }
}
