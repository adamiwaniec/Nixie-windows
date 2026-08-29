pub const HANDLE_NUM: usize = 40960;
pub const MAX_GPUS: usize = 8;

// GPU physical memory allocation constants
pub const MIN_ALLOCATION_SIZE: usize = 2 * 1024 * 1024;
pub const MAX_ALLOCATION_SIZE: usize = 128 * 1024 * 1024;

pub const CUDA_CONTROL_PLANE_RESERVATION_SIZE: usize = 256 * 1024 * 1024;
pub const CUDA_PROCESS_RESERVATION_SIZE: usize = 768 * 1024 * 1024;

// was "/tmp/nixie.sock", hardcoded in both daemon.rs and comm/init.rs.
// pipe names live in the NT object namespace and must have prefix \\.\pipe\
pub const DAEMON_PIPE_NAME: &str = r"\\.\pipe\nixie";
