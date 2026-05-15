use std::thread;

/// 把字节数格式化成更容易读的日志字符串。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    let bytes_f64 = bytes as f64;
    if bytes_f64 >= GB {
        format!("{:.2}GB", bytes_f64 / GB)
    } else if bytes_f64 >= MB {
        format!("{:.2}MB", bytes_f64 / MB)
    } else if bytes_f64 >= KB {
        format!("{:.2}KB", bytes_f64 / KB)
    } else {
        format!("{bytes}B")
    }
}

/// 解码图片是重 CPU 任务；这里用一个小型固定线程池即可。
///
/// - 太多线程会把内存峰值和线程调度成本一起放大。
/// - 太少线程则会让大量图片滚动加载时明显变慢。
///
/// 这里取一个保守值：最多 4 个 worker。
pub fn decode_worker_count() -> usize {
    thread::available_parallelism()
        .map(|parallelism| parallelism.get().min(4))
        .unwrap_or(2)
}
