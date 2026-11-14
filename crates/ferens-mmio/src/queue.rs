use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{scan_sysfs, BarOps, Chip, MappedBar, Mmio, PciDevice};
use ferens_gfx8::dispatch::{
    self, DispatchPlan, FENCE_MAGIC, FENCE_OFFSET, KERNARG_OFFSET, OUTPUT_OFFSET,
};
use ferens_gfx8::isa;
use ferens_gfx8::pm4::PacketBuilder;
use ferens_gfx8::regs;
use ferens_gfx8::ring::write_packet;

use crate::csb;
use crate::fault;
use crate::fw::{self, FirmwareSet};
use crate::golden;
use crate::gtt::HostBuffer;
use crate::hdp;
use crate::hqd;
use crate::iommu;
use crate::power;
use crate::rlc;
use crate::vm;

pub const VRAM_BASE: u64 = 0x0400_0000;
pub const RING_OFFSET: u64 = VRAM_BASE;
pub const RING_BYTES: usize = 4096;
pub const MQD_SHADOW_OFFSET: u64 = VRAM_BASE + 0x1000;
pub const MQD_SHADOW_BYTES: usize = 4096;
pub const PTE_TABLE_OFFSET: u64 = VRAM_BASE + 0x2000;
pub const PTE_TABLE_BYTES: usize = 65536;
pub const CSB_OFFSET: u64 = PTE_TABLE_OFFSET + PTE_TABLE_BYTES as u64;
pub const CSB_BYTES: usize = 8192;
pub const META_OFFSET: u64 = CSB_OFFSET + CSB_BYTES as u64;
pub const KERNEL_OFFSET: u64 = META_OFFSET + 0x1000;
pub const DEFAULT_PAGE_OFFSET: u64 = KERNEL_OFFSET + 0x1000;
pub const LAYOUT_END: u64 = DEFAULT_PAGE_OFFSET + 0x1000;
pub const VM_WINDOW_BYTES: u64 = 0x100_0000;
pub const GTT_WINDOW_BYTES: u64 = 0x100_0000;
pub const GTT_BASE_VA: u64 = VRAM_BASE + VM_WINDOW_BYTES;

pub const RPTR_REPORT_OFFSET: usize = 0;
pub const WPTR_POLL_OFFSET: usize = 4;
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(5);
pub const ACTIVE_TIMEOUT: Duration = Duration::from_millis(200);
pub const CP_SETTLE: Duration = Duration::from_micros(50);

pub struct GttBuffer {
    pub host: HostBuffer,
    pub va: u64,
}

pub struct CpQueue {
    regs: Mmio<MappedBar>,
    aperture: MappedBar,
    pub mec_fw_version: u32,
    pub rlc_fw_version: u32,
    pub mec2_loaded: bool,
    pub pg_initialized: bool,
    pub ring_dw: usize,
    pub doorbell_index: u32,
    gtt_cursor: u32,
    wp: usize,
    destroyed: bool,
}

fn bound_driver(slot_dir: &Path) -> Option<String> {
    let target = std::fs::read_link(slot_dir.join("driver")).ok()?;
    target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
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

pub fn dispatch_stream_va(grid: [u32; 3], group: [u32; 3], payload_va: u64) -> Vec<u32> {
    let plan = DispatchPlan {
        kernel_va: KERNEL_OFFSET,
        kernarg_va: payload_va + KERNARG_OFFSET as u64,
        fence_va: payload_va + FENCE_OFFSET as u64,
        output_va: payload_va + OUTPUT_OFFSET as u64,
        grid,
        group,
    };
    CpQueue::dispatch_packets(&plan)
}

pub fn dispatch_plan(grid: [u32; 3], group: [u32; 3]) -> DispatchPlan {
    DispatchPlan {
        kernel_va: KERNEL_OFFSET,
        kernarg_va: META_OFFSET + KERNARG_OFFSET as u64,
        fence_va: META_OFFSET + FENCE_OFFSET as u64,
        output_va: META_OFFSET + OUTPUT_OFFSET as u64,
        grid,
        group,
    }
}

impl CpQueue {
    pub fn create(slot_dir: &Path, chip: Chip) -> io::Result<Self> {
        if let Some(driver) = bound_driver(slot_dir) {
            return Err(io::Error::other(format!(
                "device bound to {driver}, unbind it before direct cp access"
            )));
        }
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
        if aperture_info.size < LAYOUT_END {
            return Err(io::Error::other(format!(
                "aperture bar is {:#x} bytes, need at least {:#x}",
                aperture_info.size, LAYOUT_END
            )));
        }
        let regs_bar = MappedBar::from_sysfs(slot_dir, regs_info.index)?;
        let aperture = MappedBar::from_sysfs(slot_dir, aperture_info.index)?;
        let regs = Mmio::new(regs_bar);
        let firmware = fw::load(chip)?;

        let mut queue = Self {
            regs,
            aperture,
            mec_fw_version: firmware.mec1.version,
            rlc_fw_version: firmware.rlc.version,
            mec2_loaded: firmware.mec2.is_some(),
            pg_initialized: false,
            ring_dw: RING_BYTES / 4,
            doorbell_index: hqd::DOORBELL_INDEX,
            gtt_cursor: 0,
            wp: 0,
            destroyed: false,
        };
        queue.bring_up(firmware, chip)?;
        Ok(queue)
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
        self.program_hqd()?;
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
        self.regs.write_reg_direct(addr_reg, firmware.version);
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

    fn setup_vm(&mut self) -> io::Result<()> {
        let start_pfn = (VRAM_BASE / vm::GPU_PAGE_BYTES) as u32;
        let end_pfn =
            ((VRAM_BASE + VM_WINDOW_BYTES + GTT_WINDOW_BYTES) / vm::GPU_PAGE_BYTES) as u32 - 1;
        let pages = vm::pte_page_count(start_pfn, end_pfn);
        let table_offset = (PTE_TABLE_OFFSET - VRAM_BASE) as usize;
        let config = vm::VmConfig {
            page_table_base_pfn: (PTE_TABLE_OFFSET / vm::GPU_PAGE_BYTES) as u32,
            start_pfn,
            end_pfn,
            default_page_pfn: (DEFAULT_PAGE_OFFSET / vm::GPU_PAGE_BYTES) as u32,
            pages,
        };
        vm::zero_entries(&mut self.aperture, table_offset, pages)?;
        vm::fill_identity_table(
            &mut self.aperture,
            table_offset,
            start_pfn,
            (VM_WINDOW_BYTES / vm::GPU_PAGE_BYTES) as usize,
        )?;
        vm::program_l1_l2(&self.regs);
        vm::program_context0(&self.regs, &config);
        hdp::invalidate_with_flush(&self.regs, hqd::VMID_0);
        Ok(())
    }

    fn acquire_queue(&mut self) {
        self.regs.write_reg_direct(
            hqd::SRBM_GFX_CNTL,
            hqd::srbm_gfx_cntl(hqd::MEC_ME1, hqd::PIPE_0, hqd::QUEUE_0, hqd::VMID_0),
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

    fn program_hqd(&mut self) -> io::Result<()> {
        let ring_va = RING_OFFSET;
        let meta_va = META_OFFSET;
        let rptr_addr = meta_va + RPTR_REPORT_OFFSET as u64;
        let wptr_addr = meta_va + WPTR_POLL_OFFSET as u64;

        let words = self.aperture.words_mut();
        let shadow_first = ((MQD_SHADOW_OFFSET - VRAM_BASE) / 4) as usize;
        let shadow_last = shadow_first + MQD_SHADOW_BYTES / 4;
        let ring_first = ((RING_OFFSET - VRAM_BASE) / 4) as usize;
        let ring_last = ring_first + RING_BYTES / 4;
        let meta_first = ((META_OFFSET - VRAM_BASE) / 4) as usize;
        let meta_last = meta_first + 1024;
        for word in words[ring_first..ring_last].iter_mut() {
            *word = 0;
        }
        for word in words[shadow_first..shadow_last].iter_mut() {
            *word = 0;
        }
        for word in words[meta_first..meta_last].iter_mut() {
            *word = 0;
        }

        self.acquire_queue();
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
        r.write_reg_direct(hqd::CP_HQD_VMID, hqd::VMID_0);
        let current = r.read_reg_direct(hqd::CP_HQD_PERSISTENT_STATE);
        r.write_reg_direct(hqd::CP_HQD_PERSISTENT_STATE, hqd::persistent_state(current));
        r.write_reg_direct(hqd::CP_HQD_PIPE_PRIORITY, 0);
        r.write_reg_direct(hqd::CP_HQD_QUEUE_PRIORITY, 0);
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
            hqd::doorbell_control(self.doorbell_index),
        );
        r.write_reg_direct(hqd::CP_HQD_PQ_WPTR, 0);
        r.write_reg_direct(hqd::CP_HQD_PQ_CONTROL, hqd::pq_control(self.ring_dw as u32));
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

    fn dispatch_packets(plan: &DispatchPlan) -> Vec<u32> {
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

    pub fn dispatch_stream(&mut self, grid: [u32; 3], group: [u32; 3]) -> Vec<u32> {
        let plan = dispatch_plan(grid, group);
        let descriptor = isa::buffer_descriptor(plan.output_va, 0, 4);
        {
            let aperture = self.aperture.words_mut();
            let first = ((META_OFFSET - VRAM_BASE) / 4) as usize + KERNARG_OFFSET / 4;
            aperture[first..first + 4].copy_from_slice(&descriptor);
        }
        Self::dispatch_packets(&plan)
    }

    pub fn alloc_gtt(&mut self, pages: usize) -> io::Result<GttBuffer> {
        let window_pages = (GTT_WINDOW_BYTES / vm::GPU_PAGE_BYTES) as usize;
        if self.gtt_cursor as usize + pages > window_pages {
            return Err(io::Error::other("gtt window is exhausted"));
        }
        let host = HostBuffer::lock(pages)?;
        let first_page_index = (GTT_BASE_VA / vm::GPU_PAGE_BYTES) as u32 + self.gtt_cursor;
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
            va: first_page_index as u64 * vm::GPU_PAGE_BYTES,
        })
    }

    pub fn submit(&mut self, packets: &[u32]) -> io::Result<()> {
        let aperture = self.aperture.words_mut();
        let first = ((RING_OFFSET - VRAM_BASE) / 4) as usize;
        let ring = &mut aperture[first..first + self.ring_dw];
        let next = write_packet(ring, self.wp, packets).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "packet does not fit the ring")
        })?;
        self.wp = next;
        hdp::flush(&self.regs);
        self.regs
            .bar()
            .write_u32(self.doorbell_index as usize * 4, self.wp as u32);
        Ok(())
    }

    pub fn write_pointer_dw(&self) -> u32 {
        self.wp as u32
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

    pub fn read_pointer_dw(&self) -> u32 {
        self.aperture
            .read_u32((META_OFFSET - VRAM_BASE) as usize + RPTR_REPORT_OFFSET)
    }

    pub fn meta_u32(&self, offset: usize) -> u32 {
        self.aperture
            .read_u32((META_OFFSET - VRAM_BASE) as usize + offset)
    }

    pub fn wait_fence(&self, timeout: Duration) -> io::Result<u32> {
        let started = Instant::now();
        loop {
            let fence = self.meta_u32(FENCE_OFFSET);
            if fence == FENCE_MAGIC {
                return Ok(fence);
            }
            if started.elapsed() > timeout {
                return Err(io::Error::other(format!(
                    "fence timeout after {}ms, rptr report {} dwords\n{}",
                    started.elapsed().as_millis(),
                    self.read_pointer_dw(),
                    self.status_report()
                )));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn status_report(&self) -> String {
        fault::snapshot(&self.regs).report()
    }

    pub fn destroy(&mut self) -> io::Result<()> {
        self.acquire_queue();
        let result = self.deactivate_hqd();
        self.release_queue();
        self.regs.write_reg_direct(
            hqd::CP_MEC_CNTL,
            hqd::CP_MEC_CNTL_ME1_HALT | hqd::CP_MEC_CNTL_ME2_HALT,
        );
        power::cg_off(&self.regs);
        rlc::stop(&self.regs);
        self.destroyed = true;
        result
    }
}

impl Drop for CpQueue {
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
            assert!(META_OFFSET.is_multiple_of(4096));
            assert!(KERNEL_OFFSET.is_multiple_of(4096));
            assert!(PTE_TABLE_OFFSET.is_multiple_of(4096));
            assert!(DEFAULT_PAGE_OFFSET.is_multiple_of(4096));
            assert!(LAYOUT_END <= VRAM_BASE + VM_WINDOW_BYTES);
            assert!(MQD_SHADOW_OFFSET + MQD_SHADOW_BYTES as u64 <= PTE_TABLE_OFFSET);
            assert!(PTE_TABLE_OFFSET + PTE_TABLE_BYTES as u64 <= CSB_OFFSET);
            assert!(CSB_OFFSET + CSB_BYTES as u64 <= META_OFFSET);
            assert!(META_OFFSET + 4096 <= KERNEL_OFFSET);
            assert!(DEFAULT_PAGE_OFFSET + 4096 <= VRAM_BASE + VM_WINDOW_BYTES);
            assert!(PTE_TABLE_BYTES == 8192 * 8);
            assert!(LAYOUT_END <= GTT_BASE_VA);
            assert!(csb::CSB_WORDS * 4 <= CSB_BYTES);
        };
        assert_eq!(CSB_OFFSET, 0x0401_2000);
        assert_eq!(META_OFFSET, 0x0401_4000);
        assert_eq!(KERNEL_OFFSET, 0x0401_5000);
        assert_eq!(DEFAULT_PAGE_OFFSET, 0x0401_6000);
        assert_eq!(LAYOUT_END, 0x0401_7000);
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
        assert_eq!(LAYOUT_END / vm::GPU_PAGE_BYTES, 0x4017);
    }

    #[test]
    fn plan_addresses_point_into_layout() {
        let plan = dispatch_plan([1, 1, 1], [64, 1, 1]);
        assert_eq!(plan.kernel_va, KERNEL_OFFSET);
        assert_eq!(plan.kernarg_va, META_OFFSET + KERNARG_OFFSET as u64);
        assert_eq!(plan.fence_va, META_OFFSET + FENCE_OFFSET as u64);
        assert_eq!(plan.output_va, META_OFFSET + OUTPUT_OFFSET as u64);
        assert!(plan.kernarg_va < plan.kernel_va);
    }

    #[test]
    fn vm_config_matches_layout_pages() {
        assert_eq!(PTE_TABLE_OFFSET / vm::GPU_PAGE_BYTES, 0x4002);
        assert_eq!(CSB_OFFSET / vm::GPU_PAGE_BYTES, 0x4012);
        assert_eq!(META_OFFSET / vm::GPU_PAGE_BYTES, 0x4014);
        assert_eq!(KERNEL_OFFSET / vm::GPU_PAGE_BYTES, 0x4015);
        assert_eq!(DEFAULT_PAGE_OFFSET / vm::GPU_PAGE_BYTES, 0x4016);
        assert_eq!(VRAM_BASE / vm::GPU_PAGE_BYTES, 0x4000);
    }

    #[test]
    fn stream_va_targets_payload_window() {
        let stream = dispatch_stream_va([1, 1, 1], [64, 1, 1], GTT_BASE_VA);
        let plan = DispatchPlan {
            kernel_va: KERNEL_OFFSET,
            kernarg_va: GTT_BASE_VA + KERNARG_OFFSET as u64,
            fence_va: GTT_BASE_VA + FENCE_OFFSET as u64,
            output_va: GTT_BASE_VA + OUTPUT_OFFSET as u64,
            grid: [1, 1, 1],
            group: [64, 1, 1],
        };
        assert_eq!(stream, CpQueue::dispatch_packets(&plan));
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
