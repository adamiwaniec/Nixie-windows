#[cfg(all(
    not(feature = "cuda-system"),
    not(feature = "cuda-12"),
    not(feature = "cuda-13")
))]
compile_error!(
    "one of the following features must be enabled: `cuda-system`, `cuda-12`, `cuda-13`"
);
#[cfg(all(feature = "cuda-system", feature = "cuda-12"))]
compile_error!("features `cuda-system` and `cuda-12` are mutually exclusive");
#[cfg(all(feature = "cuda-system", feature = "cuda-13"))]
compile_error!("features `cuda-system` and `cuda-13` are mutually exclusive");
#[cfg(all(feature = "cuda-12", feature = "cuda-13"))]
compile_error!("features `cuda-12` and `cuda-13` are mutually exclusive");

use cudarc::driver::sys::{CUstream, cudaError_enum};
use nixie_common::{
    shm::{AllocationTable, Shm, ShmGuard},
    shm_buffer::ShmBuffer,
    sync::IpcMutexGuard,
};
use std::sync::OnceLock;
use utils::set_device;

mod comm;
mod cu_api;
mod env_config;
mod init;
mod intercept;
mod intercept_launch;
mod intercept_sync;
mod memory;
mod schedule;
mod utils;

/*
 * Expected workflow:
 * 1. Attached process opened /dev/nvidia-uvm, we record the fd
 *  * 1.1. Some applications may open and close /dev/nvidia-uvm multiple times; we delay to use it until they truly invoke CUDA APIs
 * 2. Process called cudaMalloc, we use the latest uvmfd
 */

struct CuStreamWrapper(CUstream);
unsafe impl Send for CuStreamWrapper {}
unsafe impl Sync for CuStreamWrapper {}

impl CuStreamWrapper {
    pub fn new(device: i32) -> Self {
        set_device(device);
        let mut stream = std::ptr::null_mut();
        let res = unsafe {
            cu_api::cuStreamCreate(
                &mut stream,
                cudarc::driver::sys::CUstream_flags_enum::CU_STREAM_NON_BLOCKING as u32,
            )
        };
        if res != cudaError_enum::CUDA_SUCCESS {
            panic!("Failed to create stream: {:?}", res);
        }
        Self(stream)
    }
}

pub(crate) static GENERIC_DATA: OnceLock<GenericData> = OnceLock::new();

pub(crate) struct GenericData {
    shm: ShmGuard,
}

impl GenericData {
    /// Global mapping of device pointers and their sizes
    pub fn lock(&self, nth_table: usize) -> IpcMutexGuard<'_, AllocationTable> {
        // We always lock shared memory first
        self.shm.inner.alloc_tables[nth_table].lock()
    }

    pub fn new(path: &str) -> Self {
        info_eprintln!("Creating shared memory at {}", path);
        Self {
            shm: Shm::init_at(path, Shm::SHM_STRUCT_SIZE).expect("Failed to init shared memory"),
        }
    }
}

mod shm_buf {
    use super::ShmBuffer;
    use std::sync::OnceLock;
    pub(crate) static SHM_BUFFER: OnceLock<ShmBuffer> = OnceLock::new();
}

pub(crate) fn global_shm_buffer() -> &'static ShmBuffer {
    static GLOBAL_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let max_attempts = 10;
    loop {
        if let Some(buf) = shm_buf::SHM_BUFFER.get() {
            return buf;
        }
        if GLOBAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < max_attempts {
            std::thread::sleep(std::time::Duration::from_millis(100));
        } else {
            panic!(
                "SHM_BUFFER is not initialized after {} attempts",
                max_attempts
            );
        }
    }
}
