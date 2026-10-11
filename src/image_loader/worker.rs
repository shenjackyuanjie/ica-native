//! 固定大小的轻量线程池，专门跑图片/GIF 解码等阻塞 CPU 任务。
//!
//! 位于 `crate::image_loader` 之下，不引入额外依赖：把解码从 UI 线程挪走，
//! 并用有界队列限制在途任务数，避免“一张图一个线程”导致的线程与内存峰值失控。

use std::sync::{Arc, Mutex, mpsc};

/// 线程池里执行的任务。
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 一个非常轻量的固定线程池。
///
/// 这里不引入额外依赖，只负责把“解码图片”这样的阻塞 CPU 任务
/// 从 UI 线程挪走，并且限制并发度，避免旧实现那种“一张图一个线程”
/// 带来的线程数膨胀和内存峰值失控。
pub struct DecodeWorkerPool {
    sender: mpsc::SyncSender<Job>,
}

/// `schedule` 失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    /// 有界队列已满，暂时无法入队（可稍后重试）。
    Full,
    /// 线程池已关闭（接收端断开），无法再入队。
    Closed,
}

/// `DecodeWorkerPool` 的线程创建与任务调度。
impl DecodeWorkerPool {
    /// 创建线程池并启动 `worker_count` 个工作线程（至少 1 个）。
    ///
    /// - 参数 `thread_name_prefix`：工作线程名前缀，便于调试时识别；
    /// - 队列容量为 `worker_count * 4` 的有界通道，防止待执行任务堆积推高内存。
    pub fn new(worker_count: usize, thread_name_prefix: &'static str) -> Self {
        let worker_count = worker_count.max(1);
        // 解码任务会持有原始图片字节，队列必须有界，否则快速滚过大量图片时
        // 即使线程数固定，待执行任务仍会推高内存峰值。
        let (sender, receiver) = mpsc::sync_channel::<Job>(worker_count * 4);
        let receiver = Arc::new(Mutex::new(receiver));

        for worker_idx in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let thread_name = format!("{thread_name_prefix}-{worker_idx}");
            std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    loop {
                        let job = {
                            let receiver = receiver.lock().expect("图片解码任务接收锁被污染");
                            receiver.recv()
                        };

                        match job {
                            Ok(job) => job(),
                            Err(_) => break,
                        }
                    }
                })
                .expect("创建图片解码线程失败");
        }

        Self { sender }
    }

    /// 尝试把一个一次性任务入队；队列满或已关闭时返回对应 `ScheduleError`。
    ///
    /// 使用非阻塞 `try_send`：入队失败由调用方决定重试或报错，不在此处阻塞。
    pub fn schedule<F>(&self, job: F) -> Result<(), ScheduleError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.sender
            .try_send(Box::new(job))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => ScheduleError::Full,
                mpsc::TrySendError::Disconnected(_) => ScheduleError::Closed,
            })
    }
}
