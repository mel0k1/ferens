use std::io;

pub const KFD_PATH: &str = "/dev/kfd";

const KFD_TYPE: u32 = 0x4B;
const DIR_WRITE: u32 = 1;
const DIR_READ: u32 = 2;
const DIR_RDWR: u32 = 3;

const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (KFD_TYPE << 8) | nr
}

pub const GET_VERSION: u32 = ioc(DIR_READ, 0x01, 8);
pub const CREATE_QUEUE: u32 = ioc(DIR_RDWR, 0x02, 96);
pub const DESTROY_QUEUE: u32 = ioc(DIR_RDWR, 0x03, 8);
pub const ACQUIRE_VM: u32 = ioc(DIR_WRITE, 0x15, 8);
pub const ALLOC_MEMORY_OF_GPU: u32 = ioc(DIR_RDWR, 0x16, 40);
pub const FREE_MEMORY_OF_GPU: u32 = ioc(DIR_WRITE, 0x17, 8);
pub const MAP_MEMORY_TO_GPU: u32 = ioc(DIR_RDWR, 0x18, 24);

pub const QUEUE_TYPE_COMPUTE: u32 = 0x0;
pub const MIN_RING_SIZE: u32 = 1024;
pub const MAX_QUEUE_PERCENTAGE: u32 = 100;
pub const MAX_QUEUE_PRIORITY: u32 = 15;

pub const ALLOC_GTT: u32 = 1 << 1;
pub const ALLOC_EXECUTABLE: u32 = 1 << 30;
pub const ALLOC_WRITABLE: u32 = 1 << 31;

pub const MMAP_TYPE_DOORBELL: u64 = 3 << 62;
pub const MMAP_GPU_ID_SHIFT: u32 = 46;
pub const MMAP_GPU_ID_WIDTH: u32 = 16;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GetVersionArgs {
    pub major: u32,
    pub minor: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CreateQueueArgs {
    pub ring_base_address: u64,
    pub write_pointer_address: u64,
    pub read_pointer_address: u64,
    pub doorbell_offset: u64,
    pub ring_size: u32,
    pub gpu_id: u32,
    pub queue_type: u32,
    pub queue_percentage: u32,
    pub queue_priority: u32,
    pub queue_id: u32,
    pub eop_buffer_address: u64,
    pub eop_buffer_size: u64,
    pub ctx_save_restore_address: u64,
    pub ctx_save_restore_size: u32,
    pub ctl_stack_size: u32,
    pub sdma_engine_id: u32,
    pub metadata_ring_size: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DestroyQueueArgs {
    pub queue_id: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocMemoryOfGpuArgs {
    pub va_addr: u64,
    pub size: u64,
    pub handle: u64,
    pub mmap_offset: u64,
    pub gpu_id: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FreeMemoryOfGpuArgs {
    pub handle: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MapMemoryToGpuArgs {
    pub handle: u64,
    pub device_ids_array_ptr: u64,
    pub n_devices: u32,
    pub n_success: u32,
}

pub fn call<T>(fd: i32, request: u32, args: &mut T) -> io::Result<()> {
    let rc = unsafe {
        libc::ioctl(
            fd,
            request as libc::c_ulong,
            args as *mut T as *mut libc::c_void,
        )
    };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn ioctl_numbers_match_uapi_header() {
        assert_eq!(GET_VERSION, 0x8008_4B01);
        assert_eq!(CREATE_QUEUE, 0xC060_4B02);
        assert_eq!(DESTROY_QUEUE, 0xC008_4B03);
        assert_eq!(ACQUIRE_VM, 0x4008_4B15);
        assert_eq!(ALLOC_MEMORY_OF_GPU, 0xC028_4B16);
        assert_eq!(FREE_MEMORY_OF_GPU, 0x4008_4B17);
        assert_eq!(MAP_MEMORY_TO_GPU, 0xC018_4B18);
    }

    #[test]
    fn struct_layout_matches_uapi_header() {
        assert_eq!(size_of::<GetVersionArgs>(), 8);
        assert_eq!(size_of::<CreateQueueArgs>(), 96);
        assert_eq!(size_of::<DestroyQueueArgs>(), 8);
        assert_eq!(size_of::<AllocMemoryOfGpuArgs>(), 40);
        assert_eq!(size_of::<FreeMemoryOfGpuArgs>(), 8);
        assert_eq!(size_of::<MapMemoryToGpuArgs>(), 24);
    }
}
