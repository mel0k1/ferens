use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{scan_sysfs, BarOps, Chip, IrqWait, MappedBar, Mmio, PciDevice};
use ferens_gfx8::dispatch::{
    self, DispatchPlan, FENCE_MAGIC, FENCE_OFFSET, KERNARG_OFFSET, OUTPUT_OFFSET,
};
use ferens_gfx8::isa;
use ferens_gfx8::op;
use ferens_gfx8::pm4::PacketBuilder;
use ferens_gfx8::regs;
use ferens_gfx8::ring::write_packet;

use crate::csb;
use crate::fault;
use crate::fw::{self, FirmwareSet};
use crate::gfx;
use crate::golden;
use crate::gtt::HostBuffer;
use crate::hdp;
use crate::hqd;
use crate::iommu;
use crate::power;
use crate::reset;
use crate::rlc;
use crate::vm;

pub const VRAM_BASE: u64 = 0x0400_0000;
pub const QUEUE_SLOTS: usize = 8;
pub const RING_OFFSET: u64 = VRAM_BASE;
pub const RING_BYTES: usize = 4096;
pub const RING_STRIDE: u64 = 0x1000;
pub const MQD_SHADOW_OFFSET: u64 = RING_OFFSET + RING_STRIDE * QUEUE_SLOTS as u64;
pub const MQD_SHADOW_BYTES: usize = 4096;
pub const PTE_TABLE_OFFSET: u64 = MQD_SHADOW_OFFSET + MQD_SHADOW_BYTES as u64;
pub const PTE_TABLE_BYTES: usize = 65536;
pub const PTE1_TABLE_OFFSET: u64 = PTE_TABLE_OFFSET + PTE_TABLE_BYTES as u64;
pub const CSB_OFFSET: u64 = PTE1_TABLE_OFFSET + PTE_TABLE_BYTES as u64;
pub const CSB_BYTES: usize = 8192;
pub const KERNEL_OFFSET: u64 = CSB_OFFSET + CSB_BYTES as u64;
pub const DEFAULT_PAGE_OFFSET: u64 = KERNEL_OFFSET + 0x1000;
pub const LAYOUT_END: u64 = DEFAULT_PAGE_OFFSET + 0x1000;
pub const SLOT_META_OFFSET: u64 = LAYOUT_END;
pub const SLOT_META_STRIDE: u64 = 0x1000;
pub const SDMA_RING_OFFSET: u64 = SLOT_META_OFFSET + SLOT_META_STRIDE * QUEUE_SLOTS as u64;
pub const SDMA_RING_BYTES: usize = 4096;
pub const SDMA_INSTANCE_STRIDE: u64 = 0x2000;
pub const SDMA_META_OFFSET: u64 = SDMA_RING_OFFSET + SDMA_INSTANCE_STRIDE * 2;
pub const SDMA_META_INSTANCE_STRIDE: u64 = 0x10;
pub const SDMA_SCRATCH_OFFSET: u64 = SDMA_META_OFFSET + 0x1000;
pub const SDMA_LAYOUT_END: u64 = SDMA_SCRATCH_OFFSET + 0x1000;
pub const GFX_RING_OFFSET: u64 = SDMA_LAYOUT_END;
pub const GFX_RING_BYTES: usize = 4096;
pub const GFX_META_OFFSET: u64 = GFX_RING_OFFSET + GFX_RING_BYTES as u64;
pub const GFX_LAYOUT_END: u64 = GFX_META_OFFSET + 0x1000;
pub const IH_RING_OFFSET: u64 = GFX_LAYOUT_END;
pub const IH_RING_BYTES: usize = 4096;
pub const IH_WB_OFFSET: u64 = IH_RING_OFFSET + IH_RING_BYTES as u64;
pub const IH_LAYOUT_END: u64 = IH_WB_OFFSET + 0x1000;
pub const VM_WINDOW_BYTES: u64 = 0x100_0000;
pub const GTT_WINDOW_BYTES: u64 = 0x100_0000;
pub const GTT_BASE_VA: u64 = VRAM_BASE + VM_WINDOW_BYTES;

pub const RPTR_REPORT_OFFSET: usize = 0;
pub const WPTR_POLL_OFFSET: usize = 4;
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(5);
pub const ACTIVE_TIMEOUT: Duration = Duration::from_millis(200);
pub const CP_SETTLE: Duration = Duration::from_micros(50);
// VA вне identity-окна (VRAM_BASE..VRAM_BASE+VM_WINDOW_BYTES) и GTT-окна:
// PTE пустой, обращение рождает VM fault. VRAM_BASE = 0x04000000, окно 16 МБ,
// GTT с 0x05000000 — берём нулевую страницу, она заведомо неспапнута.
pub const FAULT_TARGET_VA: u64 = 0;
// Опрос VM_CONTEXT0_PROTECTION_FAULT_STATUS после фалящего диспетча.
pub const FAULT_POLL_TIMEOUT: Duration = Duration::from_secs(2);

pub struct GttBuffer {
    pub host: HostBuffer,
    pub va: u64,
}

pub const GFX_PIPE_PRIO_NORMAL: u32 = 0;
pub const GFX_PIPE_PRIO_HIGH: u32 = 2;
pub const GFX_QUEUE_PRIO_MAX: u32 = 15;

pub struct QueuePriority {
    pub pipe: u32,
    pub queue: u32,
}

impl QueuePriority {
    pub fn normal() -> Self {
        Self {
            pipe: GFX_PIPE_PRIO_NORMAL,
            queue: 0,
        }
    }

    pub fn high() -> Self {
        Self {
            pipe: GFX_PIPE_PRIO_HIGH,
            queue: GFX_QUEUE_PRIO_MAX,
        }
    }
}

pub struct QueueState {
    pub slot: usize,
    pub me: u32,
    pub pipe: u32,
    pub queue: u32,
    pub vmid: u32,
    pub priority: QueuePriority,
    pub ring_dw: usize,
    pub doorbell_index: u32,
    pub wp: usize,
}

pub struct CpDevice {
    regs: Mmio<MappedBar>,
    aperture: MappedBar,
    chip: Chip,
    pub mec_fw_version: u32,
    pub rlc_fw_version: u32,
    pub mec2_loaded: bool,
    pub pg_initialized: bool,
    gtt_cursor: u32,
    queues: Vec<QueueState>,
    destroyed: bool,
}

fn bound_driver(slot_dir: &Path) -> Option<String> {
    let target = std::fs::read_link(slot_dir.join("driver")).ok()?;
    target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

pub(crate) fn taken_by_driver(slot_dir: &Path) -> io::Result<()> {
    if let Some(driver) = bound_driver(slot_dir) {
        return Err(io::Error::other(format!(
            "device bound to {driver}, unbind it before direct engine access"
        )));
    }
    Ok(())
}

pub fn program_identity_context<B: BarOps>(
    regs: &Mmio<B>,
    aperture: &mut MappedBar,
    context: u32,
    table_offset: u64,
) -> io::Result<()> {
    let start_pfn = (VRAM_BASE / vm::GPU_PAGE_BYTES) as u32;
    let end_pfn =
        ((VRAM_BASE + VM_WINDOW_BYTES + GTT_WINDOW_BYTES) / vm::GPU_PAGE_BYTES) as u32 - 1;
    let pages = vm::pte_page_count(start_pfn, end_pfn);
    let table_byte_offset = (table_offset - VRAM_BASE) as usize;
    let config = vm::VmConfig {
        page_table_base_pfn: (table_offset / vm::GPU_PAGE_BYTES) as u32,
        start_pfn,
        end_pfn,
        default_page_pfn: (DEFAULT_PAGE_OFFSET / vm::GPU_PAGE_BYTES) as u32,
        pages,
    };
    vm::zero_entries(aperture, table_byte_offset, pages)?;
    vm::fill_identity_table(
        aperture,
        table_byte_offset,
        start_pfn,
        (VM_WINDOW_BYTES / vm::GPU_PAGE_BYTES) as usize,
    )?;
    vm::program_context(regs, context, &config);
    Ok(())
}

pub fn program_vm0<B: BarOps>(regs: &Mmio<B>, aperture: &mut MappedBar) -> io::Result<()> {
    program_identity_context(regs, aperture, 0, PTE_TABLE_OFFSET)?;
    vm::program_l1_l2(regs);
    hdp::invalidate_with_flush(regs, hqd::VMID_0);
    Ok(())
}

pub fn find_polaris() -> io::Result<PciDevice> {
    let devices = scan_sysfs()?;
    devices
        .into_iter()
        .find(|device| {
            matches!(
                device.chip,
                Chip::Polaris10 | Chip::Polaris11 | Chip::Polaris12
            )
        })
        .ok_or_else(|| io::Error::other("no polaris10/11/12 gpu found"))
}

pub fn slot_engine(slot: usize) -> (u32, u32, u32, u32) {
    (
        (slot / 4 + 1) as u32,
        (slot % 4) as u32,
        0,
        (hqd::DOORBELL_MEC_RING0 + slot as u32),
    )
}

pub fn slot_meta_va(slot: usize) -> u64 {
    SLOT_META_OFFSET + slot as u64 * SLOT_META_STRIDE
}

pub fn slot_ring_va(slot: usize) -> u64 {
    RING_OFFSET + slot as u64 * RING_STRIDE
}

pub fn dispatch_plan_for(grid: [u32; 3], group: [u32; 3], payload_va: u64) -> DispatchPlan {
    DispatchPlan {
        kernel_va: KERNEL_OFFSET,
        kernarg_va: payload_va + KERNARG_OFFSET as u64,
        fence_va: payload_va + FENCE_OFFSET as u64,
        output_va: payload_va + OUTPUT_OFFSET as u64,
        grid,
        group,
    }
}

pub fn dispatch_stream_va(grid: [u32; 3], group: [u32; 3], payload_va: u64) -> Vec<u32> {
    let plan = dispatch_plan_for(grid, group, payload_va);
    device_packets(&plan)
}

fn device_packets(plan: &DispatchPlan) -> Vec<u32> {
    let mut builder = PacketBuilder::new();
    for reg in [
        regs::COMPUTE_STATIC_THREAD_MGMT_SE0,
        regs::COMPUTE_STATIC_THREAD_MGMT_SE1,
        regs::COMPUTE_STATIC_THREAD_MGMT_SE2,
        regs::COMPUTE_STATIC_THREAD_MGMT_SE3,
    ] {
        builder.set_sh_reg(regs::sh_offset(reg), &[0xFFFF_FFFF]);
    }
    dispatch::push(&mut builder, plan);
    builder.into_words()
}

pub fn fence_irq_packet(fence_va: u64) -> Vec<u32> {
    let mut builder = PacketBuilder::new();
    builder.write_data(
        op::WRITE_DATA_DST_SEL_MEMORY,
        op::WRITE_DATA_ENGINE_ME,
        true,
        fence_va,
        &[FENCE_MAGIC],
    );
    builder.write_data(
        op::WRITE_DATA_DST_SEL_REGISTER,
        op::WRITE_DATA_ENGINE_ME,
        true,
        hqd::CPC_INT_STATUS as u64,
        &[hqd::COMPUTE_EOP_TRIGGER],
    );
    builder.into_words()
}

pub fn wait_fence_irq(
    get_fence: &mut dyn FnMut() -> u32,
    irq: &dyn ferens_core::IrqWait,
    timeout: Duration,
) -> io::Result<u32> {
    let started = Instant::now();
    loop {
        let fence = get_fence();
        if fence == FENCE_MAGIC {
            return Ok(fence);
        }
        let remaining = timeout.checked_sub(started.elapsed()).unwrap_or_default();
        if remaining.is_zero() {
            return Err(io::Error::other(format!(
                "fence timeout after {}ms",
                started.elapsed().as_millis()
            )));
        }
        irq.wait_interrupt(remaining)?;
        thread::sleep(Duration::from_millis(1));
    }
}

impl CpDevice {
    pub fn create(slot_dir: &Path, chip: Chip) -> io::Result<Self> {
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
        if aperture_info.size < IH_LAYOUT_END {
            return Err(io::Error::other(format!(
                "aperture bar is {:#x} bytes, need at least {:#x}",
                aperture_info.size, IH_LAYOUT_END
            )));
        }
        let regs_bar = MappedBar::from_sysfs(slot_dir, regs_info.index)?;
        let aperture = MappedBar::from_sysfs(slot_dir, aperture_info.index)?;
        let regs = Mmio::new(regs_bar);
        let firmware = fw::load(chip)?;

        let mut device = Self {
            regs,
            aperture,
            chip,
            mec_fw_version: firmware.mec1.version,
            rlc_fw_version: firmware.rlc.version,
            mec2_loaded: firmware.mec2.is_some(),
            pg_initialized: false,
            gtt_cursor: 0,
            queues: Vec::new(),
            destroyed: false,
        };
        device.bring_up(firmware, chip)?;
        Ok(device)
    }

    fn bring_up(&mut self, firmware: FirmwareSet, chip: Chip) -> io::Result<()> {
        golden::apply(&self.regs, chip)?;
        let built;
        let pg = match chip {
            Chip::Polaris11 | Chip::Polaris12 => {
                let list = firmware
                    .rlc_list
                    .as_ref()
                    .ok_or_else(|| io::Error::other("rlc firmware carries no register list"))?;
                let raster_config = self.regs.read_reg_direct(regs::PA_SC_RASTER_CONFIG);
                let raster_config_1 = self.regs.read_reg_direct(regs::PA_SC_RASTER_CONFIG_1);
                built = csb::csb_words(raster_config, raster_config_1);
                if built.len() * 4 > CSB_BYTES {
                    return Err(io::Error::other("clear state buffer does not fit"));
                }
                {
                    let words = self.aperture.words_mut();
                    let first = ((CSB_OFFSET - VRAM_BASE) / 4) as usize;
                    let last = first + built.len();
                    if last > words.len() {
                        return Err(io::Error::other("clear state buffer is outside aperture"));
                    }
                    words[first..last].copy_from_slice(&built);
                }
                self.verify_aperture(CSB_OFFSET, &built, "clear state buffer")?;
                Some(power::PgSetup {
                    csb: &built,
                    csb_va: CSB_OFFSET,
                    list,
                })
            }
            _ => None,
        };
        rlc::resume(&self.regs, &firmware.rlc, pg.as_ref())?;
        self.pg_initialized = pg.is_some();
        self.regs.write_reg_direct(
            hqd::CP_MEC_CNTL,
            hqd::CP_MEC_CNTL_ME1_HALT | hqd::CP_MEC_CNTL_ME2_HALT,
        );
        thread::sleep(CP_SETTLE);
        self.load_mec(
            hqd::CP_MEC_ME1_UCODE_ADDR,
            hqd::CP_MEC_ME1_UCODE_DATA,
            &firmware.mec1,
        )?;
        if let Some(mec2) = &firmware.mec2 {
            self.load_mec(hqd::CP_MEC_ME2_UCODE_ADDR, hqd::CP_MEC_ME2_UCODE_DATA, mec2)?;
        }
        self.regs.write_reg_direct(hqd::CP_MEC_CNTL, 0);
        thread::sleep(CP_SETTLE);

        self.regs.write_reg_direct(
            hqd::CP_MEC_DOORBELL_RANGE_LOWER,
            hqd::doorbell_range_lower(hqd::DOORBELL_KIQ),
        );
        self.regs.write_reg_direct(
            hqd::CP_MEC_DOORBELL_RANGE_UPPER,
            hqd::doorbell_range_upper(hqd::DOORBELL_MEC_RING7),
        );
        let mut status = self.regs.read_reg_direct(hqd::CP_PQ_STATUS);
        status |= hqd::CP_PQ_STATUS_DOORBELL_ENABLE;
        self.regs.write_reg_direct(hqd::CP_PQ_STATUS, status);

        let poll = self.regs.read_reg_direct(hqd::CP_PQ_WPTR_POLL_CNTL);
        self.regs.write_reg_direct(
            hqd::CP_PQ_WPTR_POLL_CNTL,
            poll & !hqd::CP_PQ_WPTR_POLL_CNTL_EN,
        );

        self.setup_vm()?;
        Ok(())
    }

    fn setup_vm(&mut self) -> io::Result<()> {
        program_identity_context(&self.regs, &mut self.aperture, 0, PTE_TABLE_OFFSET)?;
        program_identity_context(&self.regs, &mut self.aperture, 1, PTE1_TABLE_OFFSET)?;
        vm::program_l1_l2(&self.regs);
        vm::invalidate(&self.regs, hqd::VMID_0);
        vm::invalidate(&self.regs, hqd::VMID_0 + 1);
        Ok(())
    }

    fn load_mec(
        &mut self,
        addr_reg: u32,
        data_reg: u32,
        firmware: &fw::UcodeFirmware,
    ) -> io::Result<()> {
        self.regs.write_reg_direct(addr_reg, 0);
        for word in &firmware.words {
            self.regs.write_reg_direct(data_reg, *word);
        }
        let mut readback = Vec::with_capacity(firmware.words.len());
        for index in 0..firmware.words.len() {
            self.regs.write_reg_direct(addr_reg, index as u32);
            readback.push(self.regs.read_reg_direct(data_reg));
        }
        let expected = fw::crc32(&firmware.words);
        let actual = fw::crc32(&readback);
        if actual != expected {
            return Err(io::Error::other(format!(
                "mec ucode readback crc mismatch: want {expected:#010x}, got {actual:#010x}"
            )));
        }
        self.regs.write_reg_direct(addr_reg, firmware.version);
        Ok(())
    }

    fn verify_aperture(&mut self, vram_offset: u64, words: &[u32], what: &str) -> io::Result<()> {
        let first = ((vram_offset - VRAM_BASE) as usize) / 4;
        let mut readback = Vec::with_capacity(words.len());
        for index in 0..words.len() {
            readback.push(self.aperture.read_u32((first + index) * 4));
        }
        let expected = fw::crc32(words);
        let actual = fw::crc32(&readback);
        if actual != expected {
            return Err(io::Error::other(format!(
                "{what} readback crc mismatch: want {expected:#010x}, got {actual:#010x}"
            )));
        }
        Ok(())
    }

    pub fn open_queue(
        &mut self,
        slot: usize,
        vmid: u32,
        priority: QueuePriority,
    ) -> io::Result<()> {
        if slot >= QUEUE_SLOTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "slot must be 0..7",
            ));
        }
        if self.queues.iter().any(|queue| queue.slot == slot) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "slot is already open",
            ));
        }
        let (me, pipe, queue_id, doorbell_index) = slot_engine(slot);
        if me == 2 && !self.mec2_loaded {
            return Err(io::Error::other("slots 4..7 need mec2 firmware"));
        }
        let ring_va = slot_ring_va(slot);
        let meta_va = slot_meta_va(slot);
        let rptr_addr = meta_va + RPTR_REPORT_OFFSET as u64;
        let wptr_addr = meta_va + WPTR_POLL_OFFSET as u64;

        {
            let words = self.aperture.words_mut();
            let shadow_first = ((MQD_SHADOW_OFFSET - VRAM_BASE) / 4) as usize;
            let shadow_last = shadow_first + MQD_SHADOW_BYTES / 4;
            let ring_first = ((ring_va - VRAM_BASE) / 4) as usize;
            let ring_last = ring_first + RING_BYTES / 4;
            let meta_first = ((meta_va - VRAM_BASE) / 4) as usize;
            let meta_last = meta_first + SLOT_META_STRIDE as usize / 4;
            for word in words[shadow_first..shadow_last].iter_mut() {
                *word = 0;
            }
            for word in words[ring_first..ring_last].iter_mut() {
                *word = 0;
            }
            for word in words[meta_first..meta_last].iter_mut() {
                *word = 0;
            }
        }

        self.acquire_queue(me, pipe, queue_id, vmid);
        self.deactivate_hqd()?;

        let r = &self.regs;
        r.write_reg_direct(
            hqd::CP_MQD_BASE_ADDR,
            hqd::byte_addr_word(MQD_SHADOW_OFFSET),
        );
        r.write_reg_direct(
            hqd::CP_MQD_BASE_ADDR_HI,
            hqd::byte_addr_hi_word(MQD_SHADOW_OFFSET),
        );
        r.write_reg_direct(hqd::CP_HQD_ACTIVE, 0);
        r.write_reg_direct(hqd::CP_HQD_VMID, vmid);
        let current = r.read_reg_direct(hqd::CP_HQD_PERSISTENT_STATE);
        r.write_reg_direct(hqd::CP_HQD_PERSISTENT_STATE, hqd::persistent_state(current));
        r.write_reg_direct(hqd::CP_HQD_PIPE_PRIORITY, priority.pipe);
        r.write_reg_direct(hqd::CP_HQD_QUEUE_PRIORITY, priority.queue);
        r.write_reg_direct(hqd::CP_HQD_QUANTUM, hqd::quantum());
        r.write_reg_direct(hqd::CP_HQD_PQ_BASE, hqd::pq_base_word(ring_va));
        r.write_reg_direct(hqd::CP_HQD_PQ_BASE_HI, hqd::pq_base_hi_word(ring_va));
        r.write_reg_direct(
            hqd::CP_HQD_PQ_RPTR_REPORT_ADDR,
            hqd::byte_addr_word(rptr_addr),
        );
        r.write_reg_direct(
            hqd::CP_HQD_PQ_RPTR_REPORT_ADDR_HI,
            hqd::byte_addr_hi_word(rptr_addr),
        );
        r.write_reg_direct(
            hqd::CP_HQD_PQ_WPTR_POLL_ADDR,
            hqd::byte_addr_word(wptr_addr),
        );
        r.write_reg_direct(
            hqd::CP_HQD_PQ_WPTR_POLL_ADDR_HI,
            hqd::byte_addr_hi_word(wptr_addr),
        );
        r.write_reg_direct(
            hqd::CP_HQD_PQ_DOORBELL_CONTROL,
            hqd::doorbell_control(doorbell_index),
        );
        r.write_reg_direct(hqd::CP_HQD_PQ_WPTR, 0);
        r.write_reg_direct(
            hqd::CP_HQD_PQ_CONTROL,
            hqd::pq_control(RING_BYTES as u32 / 4),
        );
        r.write_reg_direct(hqd::CP_HQD_IB_BASE_ADDR, 0);
        r.write_reg_direct(hqd::CP_HQD_IB_BASE_ADDR_HI, 0);
        r.write_reg_direct(hqd::CP_HQD_IB_RPTR, 0);
        let current = r.read_reg_direct(hqd::CP_HQD_IB_CONTROL);
        r.write_reg_direct(hqd::CP_HQD_IB_CONTROL, hqd::ib_control(current));
        let current = r.read_reg_direct(hqd::CP_HQD_IQ_TIMER);
        r.write_reg_direct(hqd::CP_HQD_IQ_TIMER, hqd::iq_timer(current));
        let current = r.read_reg_direct(hqd::CP_HQD_CTX_SAVE_CONTROL);
        r.write_reg_direct(hqd::CP_HQD_CTX_SAVE_CONTROL, hqd::ctx_save_control(current));
        let current = r.read_reg_direct(hqd::CP_MQD_CONTROL);
        r.write_reg_direct(hqd::CP_MQD_CONTROL, hqd::mqd_control(current));
        r.write_reg_direct(hqd::CP_HQD_ACTIVE, hqd::CP_HQD_ACTIVE_BIT);
        self.release_queue();

        self.queues.push(QueueState {
            slot,
            me,
            pipe,
            queue: queue_id,
            vmid,
            priority,
            ring_dw: RING_BYTES / 4,
            doorbell_index,
            wp: 0,
        });
        Ok(())
    }

    fn acquire_queue(&mut self, me: u32, pipe: u32, queue: u32, vmid: u32) {
        self.regs.write_reg_direct(
            hqd::SRBM_GFX_CNTL,
            hqd::srbm_gfx_cntl(me, pipe, queue, vmid),
        );
    }

    fn release_queue(&mut self) {
        self.regs.write_reg_direct(hqd::SRBM_GFX_CNTL, 0);
    }

    fn deactivate_hqd(&mut self) -> io::Result<()> {
        if self.regs.read_reg_direct(hqd::CP_HQD_ACTIVE) & hqd::CP_HQD_ACTIVE_BIT != 0 {
            self.regs
                .write_reg_direct(hqd::CP_HQD_DEQUEUE_REQUEST, hqd::CP_HQD_DEQUEUE_RESET_WAVES);
            let started = Instant::now();
            while self.regs.read_reg_direct(hqd::CP_HQD_ACTIVE) & hqd::CP_HQD_ACTIVE_BIT != 0 {
                if started.elapsed() > ACTIVE_TIMEOUT {
                    return Err(io::Error::other("cp_hqd_active did not clear on dequeue"));
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.regs.write_reg_direct(hqd::CP_HQD_DEQUEUE_REQUEST, 0);
        self.regs.write_reg_direct(hqd::CP_HQD_PQ_RPTR, 0);
        self.regs.write_reg_direct(hqd::CP_HQD_PQ_WPTR, 0);
        Ok(())
    }

    pub fn load_kernel(&mut self, words: &[u32]) -> io::Result<()> {
        let aperture = self.aperture.words_mut();
        let first = ((KERNEL_OFFSET - VRAM_BASE) / 4) as usize;
        let last = first + words.len();
        if last > aperture.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "kernel too big",
            ));
        }
        aperture[first..last].copy_from_slice(words);
        Ok(())
    }

    pub fn dispatch_stream(&mut self, slot: usize, grid: [u32; 3], group: [u32; 3]) -> Vec<u32> {
        let payload_va = slot_meta_va(slot);
        let plan = dispatch_plan_for(grid, group, payload_va);
        let descriptor = isa::buffer_descriptor(plan.output_va, 0, 4);
        {
            let aperture = self.aperture.words_mut();
            let first = ((payload_va - VRAM_BASE) / 4) as usize + KERNARG_OFFSET / 4;
            aperture[first..first + 4].copy_from_slice(&descriptor);
        }
        device_packets(&plan)
    }

    // Диспетч, чей вывод идёт на FAULT_TARGET_VA — страницу вне identity- и
    // GTT-окон: волна умирает по TLB-промаху, CP лочит VM fault в статусах GMC.
    // Дескриптор по-прежнему кладётся в meta-область слота, фейлится только вывод.
    pub fn dispatch_fault_stream(
        &mut self,
        slot: usize,
        grid: [u32; 3],
        group: [u32; 3],
    ) -> Vec<u32> {
        let payload_va = slot_meta_va(slot);
        let mut plan = dispatch_plan_for(grid, group, payload_va);
        plan.output_va = FAULT_TARGET_VA;
        let descriptor = isa::buffer_descriptor(FAULT_TARGET_VA, 0, 4);
        {
            let aperture = self.aperture.words_mut();
            let first = ((payload_va - VRAM_BASE) / 4) as usize + KERNARG_OFFSET / 4;
            aperture[first..first + 4].copy_from_slice(&descriptor);
        }
        device_packets(&plan)
    }

    // Опрос VM_CONTEXT0_PROTECTION_FAULT_STATUS: бит VALID лочится на фолте
    // независимо от включения прерываний.
    pub fn wait_fault(&self, timeout: Duration) -> io::Result<fault::FaultInfo> {
        let started = Instant::now();
        loop {
            if let Some(fault) = fault::read_fault(&self.regs, 0) {
                return Ok(fault);
            }
            if started.elapsed() > timeout {
                return Err(io::Error::other(format!(
                    "vm fault did not arrive in {}ms (dispatch to {FAULT_TARGET_VA:#x})",
                    timeout.as_millis()
                )));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn fault_report(&self) -> String {
        match fault::read_fault(&self.regs, 0) {
            Some(info) => info.report(),
            None => "vm fault: нет зачёта в VM_CONTEXT0_PROTECTION_FAULT_STATUS".to_string(),
        }
    }

    pub fn clear_fault(&self) {
        fault::clear_fault(&self.regs, 0);
    }

    pub fn dispatch_stream_irq(
        &mut self,
        slot: usize,
        grid: [u32; 3],
        group: [u32; 3],
    ) -> Vec<u32> {
        let mut packets = self.dispatch_stream(slot, grid, group);
        packets.extend(fence_irq_packet(slot_meta_va(slot) + FENCE_OFFSET as u64));
        packets
    }

    pub fn enable_pipe_irq(&self, pipe: usize, on: bool) {
        let reg = hqd::CP_ME1_PIPE0_INT_CNTL + pipe as u32;
        let mut value = self.regs.read_reg_direct(reg);
        if on {
            value |= hqd::CP_INT_TIME_STAMP_INT_ENABLE;
        } else {
            value &= !hqd::CP_INT_TIME_STAMP_INT_ENABLE;
        }
        self.regs.write_reg_direct(reg, value);
    }

    pub fn regs(&self) -> &Mmio<MappedBar> {
        &self.regs
    }

    pub fn aperture(&self) -> &MappedBar {
        &self.aperture
    }

    pub fn aperture_mut(&mut self) -> &mut MappedBar {
        &mut self.aperture
    }

    pub fn alloc_gtt(&mut self, pages: usize) -> io::Result<GttBuffer> {
        let window_pages = (GTT_WINDOW_BYTES / vm::GPU_PAGE_BYTES) as usize;
        if self.gtt_cursor as usize + pages > window_pages {
            return Err(io::Error::other("gtt window is exhausted"));
        }
        let host = HostBuffer::lock(pages)?;
        let first_page_index = (GTT_BASE_VA / vm::GPU_PAGE_BYTES) as u32 + self.gtt_cursor;
        for table_offset in [PTE_TABLE_OFFSET, PTE1_TABLE_OFFSET] {
            vm::fill_system_entries(
                &mut self.aperture,
                (table_offset - VRAM_BASE) as usize,
                first_page_index,
                host.pfns(),
            )?;
        }
        self.gtt_cursor += pages as u32;
        hdp::invalidate_with_flush(&self.regs, hqd::VMID_0);
        hdp::invalidate_with_flush(&self.regs, hqd::VMID_0 + 1);
        Ok(GttBuffer {
            host,
            va: first_page_index as u64 * vm::GPU_PAGE_BYTES,
        })
    }

    pub fn submit(&mut self, slot: usize, packets: &[u32]) -> io::Result<()> {
        let state = self
            .queues
            .iter()
            .find(|queue| queue.slot == slot)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "slot is not open"))?;
        let ring_va = slot_ring_va(slot);
        let doorbell_index = state.doorbell_index;
        let wp = state.wp;
        let aperture = self.aperture.words_mut();
        let first = ((ring_va - VRAM_BASE) / 4) as usize;
        let ring = &mut aperture[first..first + RING_BYTES / 4];
        let next = write_packet(ring, wp, packets).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "packet does not fit the ring")
        })?;
        hdp::flush(&self.regs);
        self.regs
            .bar()
            .write_u32(doorbell_index as usize * 4, next as u32);
        if let Some(state) = self.queues.iter_mut().find(|queue| queue.slot == slot) {
            state.wp = next;
        }
        Ok(())
    }

    pub fn write_pointer_dw(&self, slot: usize) -> u32 {
        self.queues
            .iter()
            .find(|queue| queue.slot == slot)
            .map(|queue| queue.wp as u32)
            .unwrap_or(0)
    }

    pub fn read_reg(&self, offset: u32) -> u32 {
        self.regs.read_reg_direct(offset)
    }

    pub fn enable_cg(&self) {
        power::cg_on(&self.regs);
    }

    pub fn enable_pg(&self) {
        power::enable_pg(&self.regs);
    }

    pub fn read_pointer_dw(&self, slot: usize) -> u32 {
        self.aperture
            .read_u32((slot_meta_va(slot) - VRAM_BASE) as usize + RPTR_REPORT_OFFSET)
    }

    pub fn meta_u32(&self, slot: usize, offset: usize) -> u32 {
        self.aperture
            .read_u32((slot_meta_va(slot) - VRAM_BASE) as usize + offset)
    }

    pub fn wait_fence(&mut self, slot: usize, timeout: Duration) -> io::Result<u32> {
        let started = Instant::now();
        loop {
            let fence = self.meta_u32(slot, FENCE_OFFSET);
            if fence == FENCE_MAGIC {
                return Ok(fence);
            }
            if started.elapsed() > timeout {
                return Err(self.fence_timeout_error(slot, started.elapsed()));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn wait_fence_with_irq(
        &mut self,
        slot: usize,
        irq: &dyn IrqWait,
        timeout: Duration,
    ) -> io::Result<u32> {
        let outcome = {
            let mut get_fence = || self.meta_u32(slot, FENCE_OFFSET);
            wait_fence_irq(&mut get_fence, irq, timeout)
        };
        outcome
            .map_err(|error| io::Error::other(format!("{error}\n{}", self.hang_reset_report(slot))))
    }

    // Таймаут фенса: попытка автоматического dequeue-сброса
    // (gfx_v8_0_deactivate_hqd(adev, 2)) и hang-отчёт.
    fn hang_reset_report(&mut self, slot: usize) -> String {
        let hang = self.hang_report(slot);
        let reset = match self.reset_slot(slot) {
            Ok(()) => "auto dequeue reset: ok (слот закрыт, для работы нужен повторный open_queue)"
                .to_string(),
            Err(error) => format!("auto dequeue reset failed: {error}"),
        };
        format!("{reset}\n{hang}")
    }

    fn fence_timeout_error(&mut self, slot: usize, waited: Duration) -> io::Error {
        let hang_reset = self.hang_reset_report(slot);
        io::Error::other(format!(
            "fence timeout after {}ms\n{hang_reset}",
            waited.as_millis()
        ))
    }

    // Сброс зависшего слота: та же последовательность, что в deactivate_hqd,
    // но по запросу снаружи; после успеха слот выписывается из списка
    // открытых, как после destroy, и его можно открыть заново.
    pub fn reset_slot(&mut self, slot: usize) -> io::Result<()> {
        let state = self
            .queues
            .iter()
            .find(|queue| queue.slot == slot)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "slot is not open"))?;
        let (me, pipe, queue, vmid) = (state.me, state.pipe, state.queue, state.vmid);
        self.acquire_queue(me, pipe, queue, vmid);
        let result = self.deactivate_hqd();
        self.release_queue();
        if result.is_ok() {
            self.queues.retain(|state| state.slot != slot);
        }
        result
    }

    // План soft reset без исполнения — busy-маски в текущем состоянии карты.
    pub fn soft_reset_plan(&self) -> String {
        reset::plan_soft_reset(&self.regs).report()
    }

    // Полный soft reset карты: маски собираются по busy-битам
    // (gfx_v8_0_check_soft_reset + gmc_v8_0_check_soft_reset + SDMA), pre_soft_reset
    // глушит RLC/GFX/MEC и деактивирует открытые HQD, apply_soft_reset дёргает
    // GRBM/SRBM_SOFT_RESET, пост — полный ресетап как в bring_up (золотые
    // регистры, RLC/PG, MEC-микрокод, VM-таблицы). Открытые слоты и GTT-буферы
    // после сброса не выживают — их надо открывать и аллоцировать заново.
    pub fn soft_reset(&mut self) -> io::Result<String> {
        let plan = reset::plan_soft_reset(&self.regs);
        if plan.is_empty() {
            return Ok("карта простаивает (busy-биты чисты), сброс не потребовался".to_string());
        }
        // gfx_v8_0_pre_soft_reset
        rlc::stop(&self.regs);
        self.regs.write_reg_direct(
            gfx::CP_ME_CNTL,
            gfx::CP_ME_CNTL_ME_HALT | gfx::CP_ME_CNTL_PFP_HALT | gfx::CP_ME_CNTL_CE_HALT,
        );
        let slots: Vec<(u32, u32, u32, u32)> = self
            .queues
            .iter()
            .map(|queue| (queue.me, queue.pipe, queue.queue, queue.vmid))
            .collect();
        for (me, pipe, queue, vmid) in slots {
            self.acquire_queue(me, pipe, queue, vmid);
            // в pre_soft_reset результат deactivate не проверяется — HQD уйдёт
            // вместе с мягким сбросом CP
            let _ = self.deactivate_hqd();
            self.release_queue();
        }
        self.regs.write_reg_direct(
            hqd::CP_MEC_CNTL,
            hqd::CP_MEC_CNTL_ME1_HALT | hqd::CP_MEC_CNTL_ME2_HALT,
        );
        power::cg_off(&self.regs);
        // gfx_v8_0_soft_reset
        reset::apply_soft_reset(&self.regs, plan)?;
        // пост-ресетап: golden, RLC/PG, MEC, VM — как при create
        self.queues.clear();
        self.gtt_cursor = 0;
        let firmware = fw::load(self.chip)?;
        self.bring_up(firmware, self.chip)?;
        Ok(plan.report())
    }

    pub fn hang_report(&self, slot: usize) -> String {
        let (me, pipe, queue, doorbell) = slot_engine(slot);
        let mut lines = Vec::new();
        lines.push(format!(
            "slot {slot} me{me} pipe{pipe} queue{queue} doorbell {doorbell:#x}: rptr {} dwords, wptr {} dwords",
            self.read_pointer_dw(slot),
            self.write_pointer_dw(slot)
        ));
        let prev = self.regs.read_reg_direct(hqd::SRBM_GFX_CNTL);
        self.regs.write_reg_direct(
            hqd::SRBM_GFX_CNTL,
            hqd::srbm_gfx_cntl(me, pipe, queue, slot as u32),
        );
        let active = self.regs.read_reg_direct(hqd::CP_HQD_ACTIVE);
        let dequeue = self.regs.read_reg_direct(hqd::CP_HQD_DEQUEUE_REQUEST);
        self.regs.write_reg_direct(hqd::SRBM_GFX_CNTL, prev);
        lines.push(format!(
            "hqd active {active:#x}, dequeue request {dequeue:#x}"
        ));
        lines.push(self.status_report());
        lines.join("\n")
    }

    pub fn status_report(&self) -> String {
        fault::snapshot(&self.regs).report()
    }

    pub fn destroy(&mut self) -> io::Result<()> {
        let slots: Vec<(u32, u32, u32, u32)> = self
            .queues
            .iter()
            .map(|queue| (queue.me, queue.pipe, queue.queue, queue.vmid))
            .collect();
        for (me, pipe, queue, vmid) in slots {
            self.acquire_queue(me, pipe, queue, vmid);
            let result = self.deactivate_hqd();
            self.release_queue();
            result?;
        }
        self.queues.clear();
        self.regs.write_reg_direct(
            hqd::CP_MEC_CNTL,
            hqd::CP_MEC_CNTL_ME1_HALT | hqd::CP_MEC_CNTL_ME2_HALT,
        );
        power::cg_off(&self.regs);
        rlc::stop(&self.regs);
        self.destroyed = true;
        Ok(())
    }
}

impl Drop for CpDevice {
    fn drop(&mut self) {
        if !self.destroyed {
            let _ = self.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits_vm_window_and_is_page_aligned() {
        const _: () = {
            assert!(RING_BYTES.is_power_of_two());
            assert!(PTE_TABLE_BYTES == 8192 * 8);
            assert!(VRAM_BASE.is_multiple_of(4096));
            assert!(KERNEL_OFFSET.is_multiple_of(4096));
            assert!(PTE_TABLE_OFFSET.is_multiple_of(4096));
            assert!(PTE1_TABLE_OFFSET.is_multiple_of(4096));
            assert!(DEFAULT_PAGE_OFFSET.is_multiple_of(4096));
            assert!(LAYOUT_END <= VRAM_BASE + VM_WINDOW_BYTES);
            assert!(MQD_SHADOW_OFFSET + MQD_SHADOW_BYTES as u64 <= PTE_TABLE_OFFSET);
            assert!(PTE_TABLE_OFFSET + PTE_TABLE_BYTES as u64 <= PTE1_TABLE_OFFSET);
            assert!(PTE1_TABLE_OFFSET + PTE_TABLE_BYTES as u64 <= CSB_OFFSET);
            assert!(CSB_OFFSET + CSB_BYTES as u64 <= KERNEL_OFFSET);
            assert!(DEFAULT_PAGE_OFFSET + 4096 <= VRAM_BASE + VM_WINDOW_BYTES);
            assert!(LAYOUT_END <= GTT_BASE_VA);
            assert!(GFX_LAYOUT_END <= GTT_BASE_VA);
            assert!(csb::CSB_WORDS * 4 <= CSB_BYTES);
        };
        assert_eq!(MQD_SHADOW_OFFSET, 0x0400_8000);
        assert_eq!(PTE_TABLE_OFFSET, 0x0400_9000);
        assert_eq!(PTE1_TABLE_OFFSET, 0x0401_9000);
        assert_eq!(CSB_OFFSET, 0x0402_9000);
        assert_eq!(KERNEL_OFFSET, 0x0402_B000);
        assert_eq!(DEFAULT_PAGE_OFFSET, 0x0402_C000);
        assert_eq!(LAYOUT_END, 0x0402_D000);
        assert_eq!(SLOT_META_OFFSET, 0x0402_D000);
        assert_eq!(SDMA_RING_OFFSET, 0x0403_5000);
        assert_eq!(SDMA_META_OFFSET, 0x0403_9000);
        assert_eq!(SDMA_LAYOUT_END, 0x0403_B000);
        assert_eq!(GFX_RING_OFFSET, 0x0403_B000);
        assert_eq!(GFX_META_OFFSET, 0x0403_C000);
        assert_eq!(GFX_LAYOUT_END, 0x0403_D000);
        assert_eq!(IH_RING_OFFSET, 0x0403_D000);
        assert_eq!(IH_WB_OFFSET, 0x0403_E000);
        assert_eq!(IH_LAYOUT_END, 0x0403_F000);
    }

    struct ArmedIrq {
        waits: std::cell::Cell<usize>,
    }

    impl ferens_core::IrqWait for ArmedIrq {
        fn wait_interrupt(&self, _timeout: Duration) -> io::Result<()> {
            self.waits.set(self.waits.get() + 1);
            thread::sleep(Duration::from_millis(1));
            Ok(())
        }
    }

    #[test]
    fn wait_fence_irq_returns_when_fence_lands_after_interrupt() {
        let irq = ArmedIrq {
            waits: std::cell::Cell::new(0),
        };
        let mut reads = 0;
        let mut get_fence = || {
            reads += 1;
            if reads >= 2 {
                FENCE_MAGIC
            } else {
                0
            }
        };
        let fence = wait_fence_irq(&mut get_fence, &irq, Duration::from_secs(1)).unwrap();
        assert_eq!(fence, FENCE_MAGIC);
        assert_eq!(irq.waits.get(), 1);
    }

    #[test]
    fn wait_fence_irq_reports_timeout_without_firing() {
        let irq = ArmedIrq {
            waits: std::cell::Cell::new(0),
        };
        let mut get_fence = || 0u32;
        let error = wait_fence_irq(&mut get_fence, &irq, Duration::from_millis(40)).unwrap_err();
        assert!(error.to_string().contains("fence timeout"));
        assert!(irq.waits.get() > 0);
    }

    #[test]
    fn gtt_window_sits_above_identity_vram() {
        assert_eq!(VRAM_BASE, 0x0400_0000);
        assert_eq!(GTT_BASE_VA, 0x0500_0000);
        assert_eq!(GTT_BASE_VA / vm::GPU_PAGE_BYTES, 0x5000);
        let identity_pages = (VM_WINDOW_BYTES / vm::GPU_PAGE_BYTES) as u32;
        assert_eq!(identity_pages, 4096);
        let end_pfn =
            ((VRAM_BASE + VM_WINDOW_BYTES + GTT_WINDOW_BYTES) / vm::GPU_PAGE_BYTES) as u32 - 1;
        assert_eq!(end_pfn, 0x5000 + 4096 - 1);
        assert_eq!(LAYOUT_END / vm::GPU_PAGE_BYTES, 0x402D);
    }

    #[test]
    fn plan_addresses_point_into_slot_meta() {
        let plan = dispatch_plan_for([1, 1, 1], [64, 1, 1], slot_meta_va(2));
        assert_eq!(plan.kernel_va, KERNEL_OFFSET);
        assert_eq!(plan.kernarg_va, slot_meta_va(2) + KERNARG_OFFSET as u64);
        assert_eq!(plan.fence_va, slot_meta_va(2) + FENCE_OFFSET as u64);
        assert_eq!(plan.output_va, slot_meta_va(2) + OUTPUT_OFFSET as u64);
        assert!(plan.kernarg_va >= SLOT_META_OFFSET);
        assert!(plan.output_va < slot_meta_va(2) + SLOT_META_STRIDE);
    }

    #[test]
    fn vm_config_matches_layout_pages() {
        assert_eq!(PTE_TABLE_OFFSET / vm::GPU_PAGE_BYTES, 0x4009);
        assert_eq!(PTE1_TABLE_OFFSET / vm::GPU_PAGE_BYTES, 0x4019);
        assert_eq!(CSB_OFFSET / vm::GPU_PAGE_BYTES, 0x4029);
        assert_eq!(KERNEL_OFFSET / vm::GPU_PAGE_BYTES, 0x402B);
        assert_eq!(DEFAULT_PAGE_OFFSET / vm::GPU_PAGE_BYTES, 0x402C);
        assert_eq!(VRAM_BASE / vm::GPU_PAGE_BYTES, 0x4000);
    }

    #[test]
    fn slot_engine_maps_mec_pipe_and_doorbell() {
        assert_eq!(slot_engine(0), (1, 0, 0, hqd::DOORBELL_MEC_RING0));
        assert_eq!(slot_engine(3), (1, 3, 0, hqd::DOORBELL_MEC_RING0 + 3));
        assert_eq!(slot_engine(4), (2, 0, 0, hqd::DOORBELL_MEC_RING0 + 4));
        assert_eq!(slot_engine(7), (2, 3, 0, hqd::DOORBELL_MEC_RING7));
    }

    #[test]
    fn slot_meta_regions_do_not_overlap_rings() {
        for slot in 0..QUEUE_SLOTS {
            let ring = slot_ring_va(slot);
            let meta = slot_meta_va(slot);
            assert!(ring < PTE_TABLE_OFFSET);
            assert!(meta >= LAYOUT_END);
            assert!(meta + SLOT_META_STRIDE <= SDMA_RING_OFFSET);
        }
    }

    #[test]
    fn stream_va_targets_payload_window() {
        let stream = dispatch_stream_va([1, 1, 1], [64, 1, 1], GTT_BASE_VA);
        let plan = dispatch_plan_for([1, 1, 1], [64, 1, 1], GTT_BASE_VA);
        assert_eq!(stream, device_packets(&plan));
        assert_eq!(stream[29], (GTT_BASE_VA + KERNARG_OFFSET as u64) as u32);
        assert_eq!(stream[38], (GTT_BASE_VA + FENCE_OFFSET as u64) as u32);
    }

    #[test]
    fn thread_mgmt_packets_precede_dispatch_stream() {
        let mut builder = PacketBuilder::new();
        for reg in [
            regs::COMPUTE_STATIC_THREAD_MGMT_SE0,
            regs::COMPUTE_STATIC_THREAD_MGMT_SE1,
            regs::COMPUTE_STATIC_THREAD_MGMT_SE2,
            regs::COMPUTE_STATIC_THREAD_MGMT_SE3,
        ] {
            builder.set_sh_reg(regs::sh_offset(reg), &[0xFFFF_FFFF]);
        }
        let words = builder.into_words();
        assert_eq!(words.len(), 12);
        assert_eq!(words[0], 0xC001_7600);
        assert_eq!(words[1], 0x216);
        assert_eq!(words[2], 0xFFFF_FFFF);
        assert_eq!(words[3], 0xC001_7600);
        assert_eq!(words[4], 0x217);
        assert_eq!(words[5], 0xFFFF_FFFF);
        assert_eq!(words[6], 0xC001_7600);
        assert_eq!(words[7], 0x219);
        assert_eq!(words[8], 0xFFFF_FFFF);
        assert_eq!(words[9], 0xC001_7600);
        assert_eq!(words[10], 0x21A);
        assert_eq!(words[11], 0xFFFF_FFFF);
    }
}
