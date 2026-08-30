use nixie_common::shm::{Shm, ShmGuard};

use crate::error::DaemonError;

pub(crate) fn open_shm(path: String) -> Result<ShmGuard, DaemonError> {
    tracing::debug!("open_shm({})", path);
    unsafe { Shm::open_copy_at(&path, Shm::SHM_STRUCT_SIZE) }
        .map_err(|e| DaemonError::Io("open_shm() failed", e))
}
