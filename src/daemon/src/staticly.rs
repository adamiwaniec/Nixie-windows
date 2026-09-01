use nvml_wrapper::Nvml;
use std::sync::OnceLock;

static NVML: OnceLock<Nvml> = OnceLock::new();

pub(crate) fn process_name(pid: i32) -> Option<String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) }.ok()?;
        // returns null if the process already exited

    let owned = unsafe { OwnedHandle::from_raw_handle(handle.0) }; // CloseHandle on drop

    let mut buf = [0u16; 260]; // MAX_PATH
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            HANDLE(owned.as_raw_handle()),  // input
            PROCESS_NAME_WIN32,             // input
            PWSTR(buf.as_mut_ptr()),        // output

            &mut len,                       // input: capacity in chars,
                                            // output: chars written (excluding null-terminator)
        )
    }
    .ok()?;

    // full path here; /proc/<pid>/comm was just the exe name
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    std::path::Path::new(&full)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

pub fn get_nvml() -> &'static Nvml {
    NVML.get_or_init(|| {
        // the linux build had to try a list of libnvidia-ml.so paths because its
        // not on the default loader path. for windows,nvml.dll is in System32,
        // so the default init finds it
        Nvml::init().expect("Failed to initialize NVML (is the nvidia driver installed?)")
    })
}
