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

const USAGE: &str = "usage: ferens list | ferens show <slot> | ferens pm4-dump | ferens dispatch";

const RING_BYTES: usize = 4096;
const FENCE_TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => run_list(),
        Some("show") => run_show(args.get(1).map(String::as_str)),
        Some("pm4-dump") => run_pm4_dump(),
        Some("dispatch") => run_dispatch(),
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
    let Some(bar) = device.mmio_bar() else {
        eprintln!("{slot} has no memory bar 0");
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
