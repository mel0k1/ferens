use std::path::Path;

use ferens_core::{scan_sysfs, MappedBar, Mmio, SYSFS_PCI_DEVICES};
use ferens_gfx8::pm4::{event_word, PacketBuilder};
use ferens_gfx8::regs;

const USAGE: &str = "usage: ferens list | ferens show <slot> | ferens pm4-dump";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => run_list(),
        Some("show") => run_show(args.get(1).map(String::as_str)),
        Some("pm4-dump") => run_pm4_dump(),
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

fn run_pm4_dump() {
    let mut builder = PacketBuilder::new();
    builder.set_sh_reg(regs::sh_offset(regs::COMPUTE_NUM_THREAD_X), &[64, 1, 1]);
    builder.set_sh_reg(regs::sh_offset(regs::COMPUTE_PGM_RSRC1), &[0]);
    builder.set_sh_reg(regs::sh_offset(regs::COMPUTE_USER_DATA_0), &[1, 2, 3, 4]);
    builder.dispatch_direct(4, 4, 1, regs::DISPATCH_SHADER_EN);
    builder.event_write(event_word(7, 4));
    for (index, word) in builder.as_slice().iter().enumerate() {
        println!("{index:03} {word:08x}");
    }
    println!("total {} dwords", builder.as_slice().len());
}
