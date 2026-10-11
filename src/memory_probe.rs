//! 启动期内存探针：在关键阶段打印进程内存计数器，用于定位内存增长。
//!
//! 仅在 Windows 上采集真实数据（`GetProcessMemoryInfo`），其他平台退化为空操作，
//! 因此调用点（`main` / `egui_main`）无需再写 `#[cfg]`。默认关闭，设置环境变量
//! `ICA_NATIVE_MEMORY_PROBE=1` 后按阶段输出 `working_set` / `pagefile` 等指标，
//! 便于在只读日志里对照「哪个阶段之后内存开始上涨」。

/// 记录一个阶段标签及其内存快照。
///
/// `stage` 是调用点自定义的短标签（如 `cfg:snapshot`），随日志一起输出；
/// 采集失败只打印警告，不影响调用方流程。未启用探针时立即返回。
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
            "读取进程内存计数器失败"
        ),
    }
}

/// 非 Windows 平台的空实现：保持调用点写法与 Windows 一致。
#[cfg(not(windows))]
pub fn log(_stage: &str) {}

/// 是否启用探针：环境变量存在且取值不为空 / `0` / `false` / `off` / `no` 时启用。
///
/// 判断大小写不敏感，因此 `OFF`、`False` 等写法同样视为关闭。
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

/// 一次 `PROCESS_MEMORY_COUNTERS_EX` 采样中关心的字段（单位均为字节）。
#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct MemorySnapshot {
    /// 当前工作集大小，即物理内存中的占用。
    working_set: usize,
    /// 历史工作集峰值，用于观察是否存在内存泄漏式增长。
    peak_working_set: usize,
    /// 当前页面文件使用量（含被换出的部分）。
    pagefile: usize,
    /// 页面文件使用量峰值。
    peak_pagefile: usize,
    /// 私有提交量，近似进程独占的虚拟内存。
    private_usage: usize,
}

/// 读取当前进程的内存计数器；API 返回 0（失败）时得到 `None`。
///
/// 副作用：无，只读系统计数器。
#[cfg(windows)]
fn snapshot() -> Option<MemorySnapshot> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };

    // Win32 惯例：调用方先零初始化结构体，再手动填写 cb 大小字段，
    // API 依据 cb 判断调用方使用的结构体版本。
    let mut counters = unsafe { zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;

    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), counters.cb)
    };

    // Win32 布尔约定：非零表示成功，此时才把计数器搬进快照结构体。
    (ok != 0).then_some(MemorySnapshot {
        working_set: counters.WorkingSetSize,
        peak_working_set: counters.PeakWorkingSetSize,
        pagefile: counters.PagefileUsage,
        peak_pagefile: counters.PeakPagefileUsage,
        private_usage: counters.PrivateUsage,
    })
}

/// 把字节数格式化为人类可读字符串：1024 进制，自动选择 B/KB/MB/GB，保留两位小数。
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
