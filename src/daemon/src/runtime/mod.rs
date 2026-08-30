pub mod daemon;
mod daemon_server;
pub mod migration;
pub mod proc_ctl;
mod schedule;
pub mod shm;

use std::collections::HashMap;

use crate::{
    control::{ProcessMetadata, ProcessResidualData, ProcessResidualRequest},
    error::DaemonError,
};
use cudarc::driver::result::device;
pub use daemon::Daemon;
use nixie_common::{GlobalDeviceId, general::CallParameter};
pub(crate) use schedule::{ClientState, Priority, PriorityLevel};

/* removed get_user and socket_chown. they chowned the socket to the user so a daemon 
   started by root was reachable. A win32 named pipe has no filesystem entry and no
   owner to change
*/

pub(super) fn get_allowed_devices_mem(
    config: &crate::config::Config,
) -> Result<HashMap<GlobalDeviceId, u64>, DaemonError> {
    let dev_count = device::get_count().map_err(|e| DaemonError::Cuda("get dev count", e.0))?;
    let mut mem_info = HashMap::with_capacity(dev_count as usize);
    for dev_id in 0..dev_count {
        let device_handle =
            device::get(dev_id).map_err(|e| DaemonError::Cuda("get device", e.0))?;
        let mem = unsafe { device::total_mem(device_handle) }
            .map_err(|e| DaemonError::Cuda("get total memory", e.0))?;
        let mem = config
            .device_limit
            .get_bytes(GlobalDeviceId(dev_id), mem as u64);
        mem_info.insert(GlobalDeviceId(dev_id), mem);
    }
    Ok(mem_info)
}

pub(crate) enum ProcCtlReq {
    List(CallParameter<(), ProcessMetadata>),
    ListProcessResidual(CallParameter<ProcessResidualRequest, ProcessResidualData>),
}
