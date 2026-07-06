#[cfg(windows)]
pub fn log(stage: &str) {
    if !enabled() {
        return;
    }

    match snapshot() {
        Some(snapshot) => tracing::info!(
            target: "ica_native::memory_probe",
            stage,
            working_set = %format_bytes(snapshot.working_set),
            peak_working_set = %format_bytes(snapshot.peak_working_set),
            pagefile = %format_bytes(snapshot.pagefile),
            peak_pagefile = %format_bytes(snapshot.peak_pagefile),
            private_usage = %format_bytes(snapshot.private_usage),
            "memory snapshot"
        ),
        None => tracing::warn!(
            target: "ica_native::memory_probe",
            stage,
            "failed to read process memory counters"
        ),
    }
}

#[cfg(not(windows))]
pub fn log(_stage: &str) {}

#[cfg(windows)]
fn enabled() -> bool {
    std::env::var_os("ICA_NATIVE_MEMORY_PROBE").is_some_and(|value| {
        let value = value.to_string_lossy();
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        )
    })
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct MemorySnapshot {
    working_set: usize,
    peak_working_set: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private_usage: usize,
}

#[cfg(windows)]
fn snapshot() -> Option<MemorySnapshot> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };

    let mut counters = unsafe { zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;

    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), counters.cb)
    };

    (ok != 0).then_some(MemorySnapshot {
        working_set: counters.WorkingSetSize,
        peak_working_set: counters.PeakWorkingSetSize,
        pagefile: counters.PagefileUsage,
        peak_pagefile: counters.PeakPagefileUsage,
        private_usage: counters.PrivateUsage,
    })
}

#[cfg(windows)]
fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.2}GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.2}MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.2}KB", bytes / KB)
    } else {
        format!("{bytes:.0}B")
    }
}
