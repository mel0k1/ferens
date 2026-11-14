use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ferens_core::{scan_sysfs, MappedBar, Mmio, SYSFS_PCI_DEVICES};
use ferens_gfx8::dispatch::{
    self, DispatchPlan, FENCE_MAGIC, FENCE_OFFSET, KERNARG_OFFSET, OUTPUT_OFFSET,
};
use ferens_gfx8::isa;
use ferens_gfx8::pm4::PacketBuilder;
use ferens_gfx8::regs;
use ferens_kfd::{Allocator, ComputeQueue, Device, GPU_PAGE_BYTES};
use ferens_mmio::{CpQueue, Sdma};

const USAGE: &str = "usage: ferens list | ferens show <slot> | ferens pm4-dump | ferens dispatch | ferens dispatch-mmio | ferens gate [pg] | ferens dispatch-gtt | ferens status | ferens sdma-copy [bytes]";

const RING_BYTES: usize = 4096;
const FENCE_TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => run_list(),
        Some("show") => run_show(args.get(1).map(String::as_str)),
        Some("pm4-dump") => run_pm4_dump(),
        Some("dispatch") => run_dispatch(),
        Some("dispatch-mmio") => run_dispatch_mmio(),
        Some("gate") => run_gate(args.get(1).map(String::as_str)),
        Some("dispatch-gtt") => run_dispatch_gtt(),
        Some("status") => run_status(),
        Some("sdma-copy") => run_sdma_copy(args.get(1).map(String::as_str)),
        _ => eprintln!("{USAGE}"),
    }
}

fn run_list() {
    match scan_sysfs() {
        Ok(devices) if devices.is_empty() => println!("no AMD GPUs found"),
        Ok(devices) => {
            for device in &devices {
                println!(
                    "{} 0x{:04x} {} rev 0x{:02x} bars {}",
                    device.slot,
                    device.device_id,
                    device.chip.name(),
                    device.revision,
                    device.bars.len()
                );
            }
        }
        Err(error) => eprintln!("pci scan failed: {error}"),
    }
}

fn run_show(slot: Option<&str>) {
    let Some(slot) = slot else {
        eprintln!("{USAGE}");
        return;
    };
    let devices = match scan_sysfs() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("pci scan failed: {error}");
            return;
        }
    };
    let Some(device) = devices.iter().find(|candidate| candidate.slot == slot) else {
        eprintln!("{slot} is not an AMD GPU on this system");
        return;
    };
    let Some(bar) = device.regs_bar() else {
        eprintln!("{slot} has no memory bar");
        return;
    };
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(slot);
    let mapped = match MappedBar::from_sysfs(&slot_dir, bar.index) {
        Ok(mapped) => mapped,
        Err(error) => {
            eprintln!("bar map failed: {error} (root required?)");
            return;
        }
    };
    let mmio = Mmio::new(mapped);
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    println!("config_memsize: {:#x}", mmio.read_reg(regs::CONFIG_MEMSIZE));
}

fn sample_plan() -> DispatchPlan {
    DispatchPlan {
        kernel_va: 0x0000_0001_0000_0000,
        kernarg_va: 0x0000_0001_0001_0000,
        fence_va: 0x0000_0001_0001_0000 + FENCE_OFFSET as u64,
        output_va: 0x0000_0001_0001_0000 + OUTPUT_OFFSET as u64,
        grid: [1, 1, 1],
        group: [64, 1, 1],
    }
}

fn run_pm4_dump() {
    let mut builder = PacketBuilder::new();
    dispatch::push(&mut builder, &sample_plan());
    for (index, word) in builder.as_slice().iter().enumerate() {
        println!("{index:03} {word:08x}");
    }
    println!("total {} dwords", builder.as_slice().len());
}

fn run_dispatch() {
    if let Err(error) = dispatch_live() {
        eprintln!("dispatch failed: {error}");
    }
}

fn run_dispatch_mmio() {
    if let Err(error) = dispatch_mmio_live() {
        eprintln!("dispatch-mmio failed: {error}");
    }
}

fn run_gate(arg: Option<&str>) {
    if let Err(error) = gate_live(arg == Some("pg")) {
        eprintln!("gate failed: {error}");
    }
}

fn run_dispatch_gtt() {
    if let Err(error) = dispatch_gtt_live() {
        eprintln!("dispatch-gtt failed: {error}");
    }
}

fn run_status() {
    if let Err(error) = status_live() {
        eprintln!("status failed: {error}");
    }
}

fn run_sdma_copy(arg: Option<&str>) {
    if let Err(error) = sdma_copy_live(arg) {
        eprintln!("sdma-copy failed: {error}");
    }
}

fn sdma_copy_live(arg: Option<&str>) -> io::Result<()> {
    let bytes: usize = match arg {
        Some(text) => text.parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "byte count must be a number")
        })?,
        None => 4096,
    };
    if bytes == 0 || bytes > ferens_mmio::COPY_COUNT_MAX || !bytes.is_multiple_of(4) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "byte count must be a multiple of 4 in 4..0x3fffff",
        ));
    }
    let device = ferens_mmio::find_polaris()?;
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(&device.slot);
    let mut sdma = Sdma::open(&slot_dir, device.chip)?;
    println!(
        "sdma fw {:#08x} / {:#08x}, rings 2x{} bytes in vram, kick via mmio wptr",
        sdma.fw_versions[0],
        sdma.fw_versions[1],
        ferens_mmio::queue::SDMA_RING_BYTES
    );
    let pages = bytes.div_ceil(GPU_PAGE_BYTES);
    let src = sdma.alloc_gtt(pages)?;
    let dst = sdma.alloc_gtt(pages)?;
    for index in 0..bytes / 4 {
        src.host
            .write_u32(index * 4, (index as u32).wrapping_mul(0x9e37_79b1) | 1);
    }
    let scratch = ferens_mmio::sdma::scratch_va();
    println!(
        "gtt src {:#x}, dst {:#x}, vram scratch {:#x}",
        src.va, dst.va, scratch
    );
    sdma.copy(scratch, src.va, bytes)?;
    println!(
        "copy gtt->vram done, rptr {} dwords",
        sdma.read_pointer_dw(0)
    );
    sdma.copy(dst.va, scratch, bytes)?;
    println!(
        "copy vram->gtt done, rptr {} dwords",
        sdma.read_pointer_dw(0)
    );
    for index in 0..bytes / 4 {
        let expected = src.host.read_u32(index * 4);
        let actual = dst.host.read_u32(index * 4);
        if expected != actual {
            let report = sdma.status(0).report();
            sdma.destroy()?;
            return Err(io::Error::other(format!(
                "mismatch at dword {index}: want {expected:#010x}, got {actual:#010x}\n{report}"
            )));
        }
    }
    println!("verify ok: {bytes} bytes roundtripped through vram");
    println!("{}", sdma.status(0).report());
    sdma.destroy()?;
    Ok(())
}

fn status_live() -> io::Result<()> {
    let device = ferens_mmio::find_polaris()?;
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(&device.slot);
    let regs_info = device
        .regs_bar()
        .ok_or_else(|| io::Error::other("no register bar"))?;
    let mapped = MappedBar::from_sysfs(&slot_dir, regs_info.index)?;
    let mmio = Mmio::new(mapped);
    println!("{}", ferens_mmio::snapshot(&mmio).report());
    Ok(())
}

fn gate_live(with_pg: bool) -> io::Result<()> {
    let device = ferens_mmio::find_polaris()?;
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(&device.slot);
    let mut queue = CpQueue::create(&slot_dir, device.chip)?;
    println!(
        "mec fw 0x{:08x}, rlc fw 0x{:08x}, mec2 {}, csb+srm {}",
        queue.mec_fw_version, queue.rlc_fw_version, queue.mec2_loaded, queue.pg_initialized
    );
    println!(
        "before: cg ctrl {:#010x}, pg cntl {:#010x}",
        queue.read_reg(ferens_mmio::RLC_CGCG_CGLS_CTRL),
        queue.read_reg(ferens_mmio::RLC_PG_CNTL)
    );
    queue.enable_cg();
    if with_pg {
        queue.enable_pg();
    }
    println!(
        "after:  cg ctrl {:#010x}, pg cntl {:#010x}",
        queue.read_reg(ferens_mmio::RLC_CGCG_CGLS_CTRL),
        queue.read_reg(ferens_mmio::RLC_PG_CNTL)
    );
    queue.destroy()?;
    Ok(())
}

fn dispatch_gtt_live() -> io::Result<()> {
    let device = ferens_mmio::find_polaris()?;
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(&device.slot);
    let mut queue = CpQueue::create(&slot_dir, device.chip)?;
    println!(
        "mec fw 0x{:08x}, rlc fw 0x{:08x}, mec2 {}, csb+srm {}",
        queue.mec_fw_version, queue.rlc_fw_version, queue.mec2_loaded, queue.pg_initialized
    );
    queue.load_kernel(&isa::KERNEL_WORDS)?;
    let buffer = queue.alloc_gtt(1)?;
    println!("gtt page mapped at va {:#x}", buffer.va);
    let plan = DispatchPlan {
        kernel_va: ferens_mmio::KERNEL_OFFSET,
        kernarg_va: buffer.va + KERNARG_OFFSET as u64,
        fence_va: buffer.va + FENCE_OFFSET as u64,
        output_va: buffer.va + OUTPUT_OFFSET as u64,
        grid: [1, 1, 1],
        group: [64, 1, 1],
    };
    let descriptor = isa::buffer_descriptor(plan.output_va, 0, 4);
    for (index, word) in descriptor.iter().enumerate() {
        buffer.host.write_u32(KERNARG_OFFSET + index * 4, *word);
    }
    buffer.host.write_u32(FENCE_OFFSET, 0);
    let stream = ferens_mmio::dispatch_stream_va([1, 1, 1], [64, 1, 1], buffer.va);
    println!("stream {} dwords", stream.len());
    queue.submit(&stream)?;

    let started = Instant::now();
    let mut fence = buffer.host.read_u32(FENCE_OFFSET);
    while fence != FENCE_MAGIC && started.elapsed() < FENCE_TIMEOUT {
        thread::sleep(Duration::from_millis(1));
        fence = buffer.host.read_u32(FENCE_OFFSET);
    }
    if fence != FENCE_MAGIC {
        let report = queue.status_report();
        queue.destroy()?;
        return Err(io::Error::other(format!(
            "fence timeout after {}ms, rptr report {} dwords\n{report}",
            started.elapsed().as_millis(),
            queue.read_pointer_dw()
        )));
    }
    println!("fence {fence:#x} after {}ms", started.elapsed().as_millis());
    println!(
        "data[0] {:#010x}, kernel writes {:#010x} (cpu polled its own memory, no BAR read)",
        buffer.host.read_u32(OUTPUT_OFFSET),
        isa::KERNEL_MAGIC
    );
    queue.destroy()?;
    Ok(())
}

fn dispatch_mmio_live() -> io::Result<()> {
    let device = ferens_mmio::find_polaris()?;
    println!(
        "{} 0x{:04x} {} rev 0x{:02x}",
        device.slot,
        device.device_id,
        device.chip.name(),
        device.revision
    );
    let slot_dir = Path::new(SYSFS_PCI_DEVICES).join(&device.slot);
    let mut queue = CpQueue::create(&slot_dir, device.chip)?;
    println!(
        "mec fw 0x{:08x}, rlc fw 0x{:08x}, mec2 {}, ring {} dwords, queue me1 pipe0 queue0, doorbell slot {:#x}",
        queue.mec_fw_version,
        queue.rlc_fw_version,
        queue.mec2_loaded,
        queue.ring_dw,
        queue.doorbell_index
    );
    queue.load_kernel(&isa::KERNEL_WORDS)?;
    let stream = queue.dispatch_stream([1, 1, 1], [64, 1, 1]);
    println!("stream {} dwords", stream.len());
    queue.submit(&stream)?;

    let started = Instant::now();
    let fence = queue.wait_fence(FENCE_TIMEOUT)?;
    println!("fence {fence:#x} after {}ms", started.elapsed().as_millis());
    println!(
        "data[0] {:#010x}, kernel writes {:#010x}",
        queue.meta_u32(OUTPUT_OFFSET),
        isa::KERNEL_MAGIC
    );
    println!(
        "ring: wp {} dwords, rptr report {} dwords",
        queue.write_pointer_dw(),
        queue.read_pointer_dw()
    );
    queue.destroy()?;
    Ok(())
}

fn dispatch_live() -> io::Result<()> {
    let device = Device::open()?;
    println!(
        "kfd {}.{} at {}",
        device.version.0,
        device.version.1,
        ferens_kfd::KFD_PATH
    );
    let nodes = ferens_kfd::scan()?;
    let Some(node) = nodes.into_iter().next() else {
        return Err(io::Error::other("no kfd gpu nodes"));
    };
    let chip = ferens_core::Chip::from_device_id(node.device_id as u16);
    println!("gpu {} gpu_id {} {}", node.slot(), node.gpu_id, chip.name());

    let render = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(node.render_node())
        .map_err(|error| io::Error::other(format!("{}: {error}", node.render_node())))?;
    let allocator = Allocator::new(device.fd(), &render);

    let ring = allocator.alloc(
        node.gpu_id,
        RING_BYTES,
        ferens_kfd::ALLOC_GTT | ferens_kfd::ALLOC_WRITABLE,
    )?;
    let meta = allocator.alloc(
        node.gpu_id,
        GPU_PAGE_BYTES,
        ferens_kfd::ALLOC_GTT | ferens_kfd::ALLOC_WRITABLE,
    )?;
    let eop = allocator.alloc(
        node.gpu_id,
        GPU_PAGE_BYTES,
        ferens_kfd::ALLOC_GTT | ferens_kfd::ALLOC_WRITABLE,
    )?;
    let mut payload = allocator.alloc(
        node.gpu_id,
        GPU_PAGE_BYTES,
        ferens_kfd::ALLOC_GTT | ferens_kfd::ALLOC_WRITABLE,
    )?;
    let mut kernel = allocator.alloc(
        node.gpu_id,
        isa::KERNEL_WORDS.len() * 4,
        ferens_kfd::ALLOC_GTT | ferens_kfd::ALLOC_WRITABLE | ferens_kfd::ALLOC_EXECUTABLE,
    )?;

    kernel.words_mut().copy_from_slice(&isa::KERNEL_WORDS);
    let output_va = payload.va + OUTPUT_OFFSET as u64;
    let descriptor = isa::buffer_descriptor(output_va, 0, 4);
    payload.words_mut()[KERNARG_OFFSET..KERNARG_OFFSET + 4].copy_from_slice(&descriptor);
    payload.write_u32(FENCE_OFFSET, 0);

    let mut queue = ComputeQueue::create(&device, node.gpu_id, ring, meta, eop)?;
    println!(
        "queue {} ring {} dwords, doorbell slot {}",
        queue.queue_id,
        queue.ring.size / 4,
        queue.queue_id * 4
    );

    let plan = DispatchPlan {
        kernel_va: kernel.va,
        kernarg_va: payload.va + KERNARG_OFFSET as u64,
        fence_va: payload.va + FENCE_OFFSET as u64,
        output_va,
        grid: [1, 1, 1],
        group: [64, 1, 1],
    };
    let mut builder = PacketBuilder::new();
    dispatch::push(&mut builder, &plan);
    queue.submit(builder.as_slice())?;
    queue.kick();

    let started = Instant::now();
    let mut fence = payload.read_u32(FENCE_OFFSET);
    while fence != FENCE_MAGIC && started.elapsed() < FENCE_TIMEOUT {
        thread::sleep(Duration::from_millis(1));
        fence = payload.read_u32(FENCE_OFFSET);
    }
    if fence != FENCE_MAGIC {
        queue.destroy()?;
        return Err(io::Error::other(format!(
            "fence timeout after {}ms, rptr {} dwords",
            started.elapsed().as_millis(),
            queue.read_pointer_dw()
        )));
    }
    println!("fence {fence:#x} after {}ms", started.elapsed().as_millis());
    println!(
        "data[0] {:#010x}, kernel writes {:#010x}",
        payload.read_u32(OUTPUT_OFFSET),
        isa::KERNEL_MAGIC
    );
    println!(
        "ring: wp {} dwords, rptr word {} ({} dwords)",
        queue.write_pointer(),
        queue.read_pointer_words(),
        queue.read_pointer_dw()
    );
    queue.destroy()?;
    Ok(())
}
