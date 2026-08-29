use core::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::PCWSTR;

pub struct ShmBuffer {
    // control data
    // shm_path and is_creator deleted. they only existed so Drop could shm_unlink.
    // A section has no filesystem entry and dies with its last handle, so there is
    // nothing we need to unlink and nothing to remember
    mapping: HANDLE,
    // buffer
    shm_addr: u64,
    shm_size: usize,
}

impl ShmBuffer {
    pub fn new(shm_path: &str, shm_size: usize, is_creator: bool) -> Result<Self, std::io::Error> {
        let wide: Vec<u16> = shm_path.encode_utf16().chain(std::iter::once(0)).collect();
        let name = PCWSTR(wide.as_ptr());

        let size_high = (shm_size >> 32) as u32;
        let size_low = (shm_size & 0xFFFF_FFFF) as u32;

        let mapping = if is_creator {
            // INVALID_HANDLE_VALUE means page-file backed, ie anonymous shared memory.
            // size is fixed here, so there is no ftruncate step.
            unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    size_high,
                    size_low,
                    name,
                )
            }
        } else {
            // no size argument bc the openers size only shows up in MapViewOfFile below,
            // so, asking for more than the creator made fails, and asking for less
            // just maps a prefix of it. callers must agree on the size beforehand
            unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, name) }
        }
        .map_err(|_| std::io::Error::last_os_error())?;

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, shm_size) };
        if view.Value.is_null() {
            // save the error before closehandle bc it will get overwritten
            let e = std::io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(mapping);
            }
            return Err(e);
        }
        let shm_addr = view.Value as u64;

        Ok(Self {
            mapping,
            shm_size,
            shm_addr,
        })
    }

    /// # Safety
    /// The caller must ensure the shm buffer is valid
    pub unsafe fn at_offset(&self, offset: u64, size: usize) -> Option<*mut u8> {
        if offset + size as u64 > self.shm_size as u64 {
            return None;
        }
        Some((self.shm_addr + offset) as *mut u8)
    }

    pub fn size(&self) -> usize {
        self.shm_size
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.shm_addr as *mut c_void,
            });
            let _ = CloseHandle(self.mapping);
        }
    }
}

// HANDLE is a process-wide token, so any thread can use it. these were derived
// automatically on Linux, where the fields were i32 and u64
unsafe impl Send for ShmBuffer {}
unsafe impl Sync for ShmBuffer {}
