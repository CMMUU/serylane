//! UI reads never execute filesystem/process work on the native event loop.
//! A timed-out task keeps its permit until it really exits; no unbounded queue.
use crate::error::{AppError, AppErrorDto};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;

pub async fn run<T, F>(work: F) -> Result<T, AppErrorDto>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppErrorDto> + Send + 'static,
{
    static READS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let slots = READS.get_or_init(|| Arc::new(Semaphore::new(4))).clone();
    let permit = tokio::time::timeout(Duration::from_secs(2), slots.acquire_owned())
        .await
        .map_err(|_| AppError::Runtime("状态读取繁忙，请稍后刷新；未更改代理设置".into()).dto())?
        .map_err(|_| AppError::Runtime("状态读取已停止".into()).dto())?;
    let job = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    tokio::time::timeout(Duration::from_secs(10), job)
        .await
        .map_err(|_| AppError::Runtime("状态读取超时，请稍后刷新；未更改代理设置".into()).dto())?
        .map_err(|_| AppError::Runtime("状态读取中断，请重新刷新".into()).dto())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn blocking_read_does_not_block_the_async_caller() {
        let caller = std::thread::current().id();
        let worker = run(move || {
            std::thread::sleep(Duration::from_millis(80));
            Ok(std::thread::current().id())
        });
        let tick = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            1
        };
        let (worker, tick) = tokio::join!(worker, tick);
        assert_ne!(caller, worker.unwrap());
        assert_eq!(tick, 1);
    }
    #[tokio::test]
    async fn concurrency_is_bounded_even_for_many_readers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut jobs = Vec::new();
        for _ in 0..20 {
            let active = active.clone();
            let peak = peak.clone();
            jobs.push(tokio::spawn(run(move || {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(15));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })));
        }
        for job in jobs {
            job.await.unwrap().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn propagates_read_error_without_retrying_work() {
        let result: Result<(), _> = run(|| Err(AppError::Io("read failure".into()).dto())).await;
        assert_eq!(result.unwrap_err().code, "IO_ERROR");
    }
}
