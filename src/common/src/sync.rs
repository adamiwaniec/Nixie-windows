use core::{cell::UnsafeCell, sync::atomic::AtomicU8};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};
use windows::core::PCWSTR;

use crate::shm::ReInitializable;

// lives in shared mem
pub struct IpcMutex<T> {
    /*  sem_t used to live here bc it can be managed in shared mem from user-space.
        but win32 mutex is a kernel obj, so we store an id that can be used to
        derive the same name
    */
    id: u64,
    ref_count: AtomicU8,
    inner: UnsafeCell<T>,
}

impl<T> IpcMutex<T> {
    // CreateMutexW is create-or-open so both processes just call this func
    fn open_mutex(&self) -> HANDLE {
        //Local\ is the per-session namespace
        // this needs Global once daemon runs as a service (session 0)
        let name = format!("Local\\nixie-mtx-{:016x}", self.id);
        let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
        unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }.expect("CreateMutexW failed")
    }

    pub fn increase_ref_count(&self) {
        self.ref_count
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }

    pub fn lock(&'_ self) -> IpcMutexGuard<'_, T> {
        let handle = self.open_mutex();
        match unsafe { WaitForSingleObject(handle, INFINITE) } {
            WAIT_OBJECT_0 => {}

            // we get the lock since owner died holding it.
            // in the old linux sem_wait, a dead owner just held the semaphore forever.
            WAIT_ABANDONED => {
                eprintln!(
                    "NIXIE-WARN: IpcMutex {:#x} abandoned by a dead owner",
                    self.id
                );
            }
            other => panic!(
                "WaitForSingleObject on IpcMutex {:#x} failed: {:?}, last error {}",
                self.id,
                other,
                std::io::Error::last_os_error()
            ),
        }
        IpcMutexGuard { lock: self, handle }
    }

    /// # Safety
    /// self must point into a mapped region big enough
    pub(crate) unsafe fn reinit_with_id(&mut self, id: u64)
    // id is passed in since only the creator knows the region name that it hashes from.
    // openers will just read it back out of the mapping.
    where
        T: ReInitializable,
    {
        self.id = id;
        self.ref_count = AtomicU8::new(1);
        unsafe { self.inner.get_mut().reinit_from_uninited() };
    }

    /// # Safety
    ///
    /// drops a ref to the lock
    pub unsafe fn close(&mut self) {
        // no sem_destroy equivalent. kernel will free the mutex once the last handle
        // closes, and the guard already closes ours.
        // the refcount is only kept for the callers that still read it
        self.ref_count
            .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
}

unsafe impl<T: Sync> Sync for IpcMutex<T> {}

pub struct IpcMutexGuard<'a, T> {
    lock: &'a IpcMutex<T>,
    handle: HANDLE,
}

impl<T> Drop for IpcMutexGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            if let Err(e) = ReleaseMutex(self.handle) {
                eprintln!("NIXIE-WARN: ReleaseMutex failed: {e}");
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

impl<T> core::ops::Deref for IpcMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.inner.get() }
    }
}

impl<T> core::ops::DerefMut for IpcMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.inner.get() }
    }
}
