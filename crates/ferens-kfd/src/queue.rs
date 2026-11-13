use std::io;
use std::sync::atomic::{fence, Ordering};

use crate::device::Device;
use crate::ioctl::{self, CreateQueueArgs, DestroyQueueArgs};
use crate::memory::GttAlloc;

pub const DOORBELL_SLICE_BYTES: usize = 4096;
pub const DOORBELL_SLOT_BYTES: usize = 4;
pub const WRITE_POINTER_OFFSET: usize = 0;
pub const READ_POINTER_OFFSET: usize = 4;
pub const RPTR_BLOCK_BYTES: u32 = 32;
pub const QUEUE_PERCENTAGE: u32 = 100;
pub const QUEUE_PRIORITY: u32 = 10;
pub const PACKET_NOP: u32 = 0xC000_1000;

pub struct Doorbell {
    ptr: *mut u32,
    bytes: usize,
}

unsafe impl Send for Doorbell {}
unsafe impl Sync for Doorbell {}

impl Doorbell {
    pub fn map(fd: i32, offset: u64) -> io::Result<Self> {
        let base = offset & !(DOORBELL_SLICE_BYTES as u64 - 1);
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                DOORBELL_SLICE_BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                base as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast::<u32>(),
            bytes: DOORBELL_SLICE_BYTES,
        })
    }

    fn slot(&self, index: usize) -> *mut u32 {
        assert!((index + 1) * DOORBELL_SLOT_BYTES <= self.bytes);
        unsafe { self.ptr.add(index) }
    }
}

impl Drop for Doorbell {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr.cast::<libc::c_void>(), self.bytes) };
    }
}

pub struct ComputeQueue {
    fd: i32,
    pub queue_id: u32,
    pub ring: GttAlloc,
    pub meta: GttAlloc,
    pub eop: GttAlloc,
    wp: usize,
    doorbell: Doorbell,
    destroyed: bool,
}

pub fn write_packet(ring: &mut [u32], wp: usize, packet: &[u32]) -> Option<usize> {
    if packet.is_empty() || packet.len() > ring.len() {
        return None;
    }
    let mut wp = wp;
    if ring.len() - wp < packet.len() {
        for word in &mut ring[wp..] {
            *word = PACKET_NOP;
        }
        wp = 0;
    }
    ring[wp..wp + packet.len()].copy_from_slice(packet);
    Some((wp + packet.len()) % ring.len())
}

impl ComputeQueue {
    pub fn create(
        device: &Device,
        gpu_id: u32,
        ring: GttAlloc,
        meta: GttAlloc,
        eop: GttAlloc,
    ) -> io::Result<Self> {
        if ring.size < ioctl::MIN_RING_SIZE as usize || !(ring.size / 4).is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ring size must be a power of two",
            ));
        }
        let mut args = CreateQueueArgs {
            ring_base_address: ring.va,
            write_pointer_address: meta.va + WRITE_POINTER_OFFSET as u64,
            read_pointer_address: meta.va + READ_POINTER_OFFSET as u64,
            ring_size: ring.size as u32,
            gpu_id,
            queue_type: ioctl::QUEUE_TYPE_COMPUTE,
            queue_percentage: QUEUE_PERCENTAGE,
            queue_priority: QUEUE_PRIORITY,
            eop_buffer_address: eop.va,
            eop_buffer_size: eop.size as u64,
            ..Default::default()
        };
        ioctl::call(device.fd(), ioctl::CREATE_QUEUE, &mut args)?;
        let doorbell = Doorbell::map(device.fd(), args.doorbell_offset)?;
        Ok(Self {
            fd: device.fd(),
            queue_id: args.queue_id,
            ring,
            meta,
            eop,
            wp: 0,
            doorbell,
            destroyed: false,
        })
    }

    pub fn submit(&mut self, packet: &[u32]) -> io::Result<()> {
        let words = self.ring.words_mut();
        let Some(next) = write_packet(words, self.wp, packet) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet does not fit the ring",
            ));
        };
        self.wp = next;
        Ok(())
    }

    pub fn kick(&mut self) {
        let wp = self.wp as u32;
        self.meta.write_u32(WRITE_POINTER_OFFSET, wp);
        fence(Ordering::Release);
        unsafe {
            std::ptr::write_volatile(self.doorbell.slot(self.queue_id as usize), wp);
        }
    }

    pub fn write_pointer(&self) -> u32 {
        self.wp as u32
    }

    pub fn read_pointer_words(&self) -> u32 {
        self.meta.read_u32(READ_POINTER_OFFSET)
    }

    pub fn read_pointer_dw(&self) -> u32 {
        self.read_pointer_words() * (RPTR_BLOCK_BYTES / 4)
    }

    pub fn destroy(&mut self) -> io::Result<()> {
        let mut args = DestroyQueueArgs {
            queue_id: self.queue_id,
            pad: 0,
        };
        let result = ioctl::call(self.fd, ioctl::DESTROY_QUEUE, &mut args);
        if result.is_ok() {
            self.destroyed = true;
        }
        result
    }
}

impl Drop for ComputeQueue {
    fn drop(&mut self) {
        if !self.destroyed {
            let mut args = DestroyQueueArgs {
                queue_id: self.queue_id,
                pad: 0,
            };
            let _ = ioctl::call(self.fd, ioctl::DESTROY_QUEUE, &mut args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_packet_fills_wrap_with_single_word_nops() {
        let mut ring = vec![0u32; 8];
        let wp = write_packet(&mut ring, 6, &[0xC003_1500, 8, 1, 1, 1]).unwrap();
        assert_eq!(wp, 5);
        assert_eq!(ring[6], PACKET_NOP);
        assert_eq!(ring[7], PACKET_NOP);
        assert_eq!(&ring[..5], &[0xC003_1500, 8, 1, 1, 1]);
    }

    #[test]
    fn write_packet_wraps_write_pointer_at_exact_fit() {
        let mut ring = vec![0u32; 4];
        let wp = write_packet(&mut ring, 0, &[0xC002_7600, 0x20C, 0xAABB, 0xCCDD]).unwrap();
        assert_eq!(wp, 0);
        let wp = write_packet(&mut ring, wp, &[0xC000_1000]).unwrap();
        assert_eq!(wp, 1);
    }

    #[test]
    fn write_packet_rejects_oversized_and_empty() {
        let mut ring = vec![0u32; 4];
        assert!(write_packet(&mut ring, 0, &[0; 5]).is_none());
        assert!(write_packet(&mut ring, 0, &[]).is_none());
    }

    #[test]
    fn read_pointer_words_scale_to_dwords() {
        assert_eq!(2u32 * (RPTR_BLOCK_BYTES / 4), 16);
    }
}
