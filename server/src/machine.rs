//! 本机配置：CPU 逻辑核数、物理内存总量。设置页按它给滑块定上限，并行编译数也以它封顶。

/// 逻辑核数；取不到按 1 算
pub fn cpus() -> u64 {
    std::thread::available_parallelism().map(|n| n.get() as u64).unwrap_or(1)
}

/// 物理内存总量（MB）；取不到为 `None`
pub fn memory_mb() -> Option<u64> {
    total_bytes().filter(|&b| b > 0).map(|b| b >> 20)
}

#[cfg(target_os = "macos")]
fn total_bytes() -> Option<u64> {
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let rc = unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0)
    };
    (rc == 0).then_some(v)
}

#[cfg(target_os = "linux")]
fn total_bytes() -> Option<u64> {
    let mut info: libc::sysinfo = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::sysinfo(&mut info) };
    (rc == 0).then(|| info.totalram as u64 * info.mem_unit.max(1) as u64)
}

#[cfg(windows)]
fn total_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut st) };
    (ok != 0).then_some(st.ullTotalPhys)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn total_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_this_machine() {
        assert!(super::cpus() >= 1);
        #[cfg(any(target_os = "macos", target_os = "linux", windows))]
        assert!(super::memory_mb().is_some_and(|mb| mb >= 256));
    }
}
