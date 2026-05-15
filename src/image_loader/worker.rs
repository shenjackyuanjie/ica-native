use std::sync::{Arc, Mutex, mpsc};

/// 线程池里执行的任务。
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 一个非常轻量的固定线程池。
///
/// 这里不引入额外依赖，只负责把“解码图片”这样的阻塞 CPU 任务
/// 从 UI 线程挪走，并且限制并发度，避免旧实现那种“一张图一个线程”
/// 带来的线程数膨胀和内存峰值失控。
pub struct DecodeWorkerPool {
    sender: mpsc::Sender<Job>,
}

impl DecodeWorkerPool {
    pub fn new(worker_count: usize, thread_name_prefix: &'static str) -> Self {
        let worker_count = worker_count.max(1);
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        for worker_idx in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let thread_name = format!("{thread_name_prefix}-{worker_idx}");
            std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    loop {
                        let job = {
                            let receiver =
                                receiver.lock().expect("decode worker receiver poisoned");
                            receiver.recv()
                        };

                        match job {
                            Ok(job) => job(),
                            Err(_) => break,
                        }
                    }
                })
                .expect("failed to spawn image decode worker");
        }

        Self { sender }
    }

    pub fn schedule<F>(&self, job: F) -> Result<(), &'static str>
    where
        F: FnOnce() + Send + 'static,
    {
        self.sender
            .send(Box::new(job))
            .map_err(|_| "image decode worker pool is shut down")
    }
}
