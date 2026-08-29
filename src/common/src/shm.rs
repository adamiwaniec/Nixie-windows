use core::{ffi::c_void, pin::Pin};
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{HANDLE_NUM, MAX_GPUS, sync::IpcMutex};

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::PCWSTR;

/// There should be no side effects of the drop.
pub(crate) trait ReInitializable {
    // receiver is freshly mapped and uninitialized so its fields hold
    // arbitrary data. overwrite them, never read or drop them. call one time
    unsafe fn reinit_from_uninited(&mut self);
}

pub struct AllocationTable {
    // usize
    pub entry: ShmVec<AllocationEntry, 8192>,
    pub handle_list: HandleList,
}

pub struct HandleList {
    // NonZeroU32
    handles: [PhysicalMemoryHandle; HANDLE_NUM],
    freelist_head: Option<NonZeroU32>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PhysicalMemoryHandleId {
    pub alloc_generation: u32, // used to identify A-B-A allocations
    pub idx: NonZeroU32,
}

impl PhysicalMemoryHandleId {
    pub fn new(alloc_generation: u32, idx: NonZeroU32) -> Self {
        Self {
            alloc_generation,
            idx,
        }
    }
}

impl AllocationTable {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            entry: ShmVec::new(),
            handle_list: HandleList::new(),
        }
    }
}

impl ReInitializable for AllocationTable {
    unsafe fn reinit_from_uninited(&mut self) {
        unsafe {
            self.entry.reinit();
            self.handle_list.reinit_from_uninited();
        }
    }
}
impl HandleList {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut handles = [PhysicalMemoryHandle {
            addr: 0,
            size: 0,
            cu_handle: None,
            alloc_generation: 0,
            next_handle_idx: None,
            on_gpu: false,
            valid: false,
        }; HANDLE_NUM];
        #[allow(clippy::needless_range_loop)]
        for i in 1..HANDLE_NUM {
            handles[i].next_handle_idx = NonZeroU32::new(i as u32 + 1);
        }
        Self {
            handles,
            freelist_head: NonZeroU32::new(1),
        }
    }

    pub fn allocate_handle(&mut self, addr: u64, size: usize) -> Option<PhysicalMemoryHandleId> {
        if let Some(idx) = self.freelist_head {
            let handle = &mut self.handles[idx.get() as usize];
            self.freelist_head = handle.next_handle_idx;
            handle.next_handle_idx = None;
            handle.alloc_generation = handle.alloc_generation.wrapping_add(1);
            handle.addr = addr;
            handle.size = size;
            handle.on_gpu = false;
            handle.valid = true;
            Some(PhysicalMemoryHandleId::new(handle.alloc_generation, idx))
        } else {
            None
        }
    }

    pub fn free_handle(&mut self, idx: PhysicalMemoryHandleId) {
        self.free_handle_by_raw_idx(idx.idx);
    }

    pub fn free_handle_by_raw_idx(&mut self, idx: NonZeroU32) {
        let handle = &mut self.handles[idx.get() as usize];
        handle.addr = 0;
        handle.size = 0;
        handle.next_handle_idx = self.freelist_head;
        handle.on_gpu = false;
        handle.valid = false;
        // we do not modify alloc_generation here
        self.freelist_head = Some(idx);
    }

    pub fn get_handle(&self, idx: PhysicalMemoryHandleId) -> Option<&PhysicalMemoryHandle> {
        self.get_handle_by_raw_idx(idx.idx)
    }

    pub fn get_handle_by_raw_idx(&self, idx: NonZeroU32) -> Option<&PhysicalMemoryHandle> {
        if idx.get() as usize >= HANDLE_NUM {
            return None;
        }
        Some(&self.handles[idx.get() as usize])
    }

    pub fn get_handle_mut(
        &mut self,
        idx: PhysicalMemoryHandleId,
    ) -> Option<&mut PhysicalMemoryHandle> {
        self.get_handle_by_raw_idx_mut(idx.idx)
    }

    pub fn get_handle_by_raw_idx_mut(
        &mut self,
        idx: NonZeroU32,
    ) -> Option<&mut PhysicalMemoryHandle> {
        if idx.get() as usize >= HANDLE_NUM {
            return None;
        }
        Some(&mut self.handles[idx.get() as usize])
    }

    // return (on_gpu, not_on_gpu)
    pub fn memory_usage(&self, handle_idx: PhysicalMemoryHandleId) -> (usize, usize) {
        let mut on_gpu = 0;
        let mut not_on_gpu = 0;
        let mut cur_index = Some(handle_idx.idx);
        while let Some(index) = cur_index {
            let handle = self.get_handle_by_raw_idx(index).unwrap();
            if handle.on_gpu {
                on_gpu += handle.size;
            } else {
                not_on_gpu += handle.size;
            }
            cur_index = handle.next_handle_idx;
        }
        (on_gpu, not_on_gpu)
    }
}

impl ReInitializable for HandleList {
    unsafe fn reinit_from_uninited(&mut self) {
        for handle in self.handles.iter_mut() {
            handle.addr = 0;
            handle.size = 0;
            handle.cu_handle = None;
            handle.alloc_generation = 0;
            handle.next_handle_idx = None;
            handle.on_gpu = false;
            handle.valid = false;
        }
        for i in 1..HANDLE_NUM {
            self.handles[i].next_handle_idx = NonZeroU32::new(i as u32 + 1);
        }
        self.freelist_head = NonZeroU32::new(1);
    }
}

pub struct Shm {
    all_len: u32,
    pub alloc_tables: [IpcMutex<AllocationTable>; MAX_GPUS], // Process local device ID
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalMemoryHandle {
    pub addr: u64,
    pub size: usize,
    pub cu_handle: Option<cudarc::driver::sys::CUmemGenericAllocationHandle>,
    pub alloc_generation: u32, // used to identify A-B-A allocations
    pub next_handle_idx: Option<NonZeroU32>,
    pub on_gpu: bool,
    pub valid: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AllocationEntry {
    pub addr: u64,
    pub len: usize,
    pub handle_idx: PhysicalMemoryHandleId,
}

impl Default for AllocationEntry {
    fn default() -> Self {
        Self {
            addr: 0,
            len: 0,
            handle_idx: PhysicalMemoryHandleId::new(u32::MAX, NonZeroU32::new(u32::MAX).unwrap()),
        }
    }
}

fn region_hash(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

impl Shm {
    pub const SHM_STRUCT_SIZE: u32 = core::mem::size_of::<Self>() as u32;

    pub fn init_at(name: &str, len: u32) -> Result<ShmGuard, std::io::Error> {
        if len < Self::SHM_STRUCT_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shm region smaller than Shm",
            ));
        }

        let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
        // len is a u32 so the high dword is always 0
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                len,
                PCWSTR(wide.as_ptr()),
            )
        }
        .map_err(|_| std::io::Error::last_os_error())?;

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len as usize) };
        if view.Value.is_null() {
            let e = std::io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(mapping);
            };
            return Err(e);
        }

        // 1 hash per region and 1 id per lock derived off it, so the locks in a region 
        // cannot collide with each other or with another region
        let hash = region_hash(name);
        unsafe {
            let mut_ref = &mut *(view.Value as *mut Self);
            mut_ref.all_len = len;
            for (i, table) in mut_ref.alloc_tables.iter_mut().enumerate() {
                table.reinit_with_id(hash.wrapping_mul(MAX_GPUS as u64) + i as u64);
            }
            Ok(ShmGuard {
                inner: Pin::new(mut_ref),
                mapping,
            })
        }
    }

    /// # Safety
    /// maps shared memory. region must have been initialized by init_at()
    pub unsafe fn open_copy_at(name: &str, len: u32) -> Result<ShmGuard, std::io::Error> {
        if len < Self::SHM_STRUCT_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shm region smaller than Shm",
            ));
        }

        let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
        let mapping =
            unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(wide.as_ptr())) }
                .map_err(|_| std::io::Error::last_os_error())?;

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len as usize) };
        if view.Value.is_null() {
            let e = std::io::Error::last_os_error();
            unsafe {
                let _ = CloseHandle(mapping);
            };
            return Err(e);
        }

        unsafe {
            let mut_ref = &mut *(view.Value as *mut Self);
            for alloc_table in mut_ref.alloc_tables.iter() {
                alloc_table.increase_ref_count();
            }
            Ok(ShmGuard {
                inner: Pin::new(mut_ref),
                mapping,
            })
        }
    }

    unsafe fn close(&mut self) {
        unsafe {
            for alloc_table in self.alloc_tables.iter_mut() {
                // decrement ref count for each allocation table
                alloc_table.close();
            }
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self as *const Self as *mut c_void,
            });
        }
    }
}

pub struct ShmGuard {
    pub inner: Pin<&'static mut Shm>,
    mapping: HANDLE,
}

impl Drop for ShmGuard {
    fn drop(&mut self) {
        unsafe {
            self.inner.close(); // refcounts, then unmaps
            let _ = CloseHandle(self.mapping);
        }
    }
}

// same as ShmBuffer. the handle is process-wide, not owned by one thr
unsafe impl Send for ShmGuard {}
unsafe impl Sync for ShmGuard {}

pub struct ShmVec<T: Default, const N: usize> {
    len: u32,
    data: [T; N],
}

impl<T: Default, const N: usize> ShmVec<T, N> {
    pub fn new() -> Self {
        Self {
            len: 0,
            data: unsafe { core::mem::zeroed() },
        }
    }

    pub fn reinit(&mut self) {
        self.len = 0;
        self.data = unsafe { core::mem::zeroed() };
    }

    #[allow(clippy::result_unit_err)]
    pub fn push(&mut self, val: T) -> Result<usize, ()> {
        if self.len as usize >= N {
            return Err(());
        }
        let new_idx = self.len as usize;
        self.data[new_idx] = val;
        self.len += 1;
        Ok(new_idx)
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(core::mem::take(&mut self.data[self.len as usize]))
    }

    pub fn remove(&mut self, idx: usize) -> T {
        if idx >= self.len as usize {
            panic!("index out of bounds")
        }
        let val = core::mem::take(&mut self.data[idx]);
        self.len -= 1;
        for i in idx..self.len as usize {
            self.data[i] = core::mem::take(&mut self.data[i + 1]);
        }
        val
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data[..self.len as usize]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data[..self.len as usize]
    }

    pub fn at(&self, idx: usize) -> Option<&T> {
        self.as_slice().get(idx)
    }

    pub fn at_mut(&mut self, idx: usize) -> Option<&mut T> {
        self.as_mut_slice().get_mut(idx)
    }

    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }
}

impl<T: Default, const N: usize> Default for ShmVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
