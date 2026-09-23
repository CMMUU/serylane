//! Platform-independent coordination for Helper reads and owned heartbeats.
#[cfg(any(target_os = "macos", test))]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(any(target_os = "macos", test))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeRead<T> {
    Complete(Result<T, String>),
    Checking,
}

#[cfg(any(target_os = "macos", test))]
struct Flight<T> {
    generation: u64,
    result: Mutex<Option<(Instant, Result<T, String>)>>,
    completed: Condvar,
}

#[cfg(any(target_os = "macos", test))]
struct ProbeState<T> {
    generation: u64,
    flight: Option<Arc<Flight<T>>>,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) struct SharedProbe<T> {
    state: Mutex<ProbeState<T>>,
}

#[cfg(any(target_os = "macos", test))]
impl<T> Default for SharedProbe<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(ProbeState {
                generation: 0,
                flight: None,
            }),
        }
    }
}

#[cfg(any(target_os = "macos", test))]
impl<T: Clone + Send + 'static> SharedProbe<T> {
    pub(crate) fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.generation = state.generation.wrapping_add(1);
            state.flight = None;
        }
    }

    pub(crate) fn read(
        &self,
        timeout: Duration,
        cache_ttl: Duration,
        operation: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> ProbeRead<T> {
        let started = Instant::now();
        let (flight, start) = {
            let Ok(mut state) = self.state.lock() else {
                return ProbeRead::Complete(Err("TUN Helper 状态检查锁损坏".to_string()));
            };
            let reusable = state.flight.as_ref().is_some_and(|flight| {
                flight.result.lock().is_ok_and(|result| {
                    result
                        .as_ref()
                        .is_none_or(|(finished, _)| finished.elapsed() < cache_ttl)
                })
            });
            if reusable {
                (state.flight.as_ref().unwrap().clone(), false)
            } else {
                let flight = Arc::new(Flight {
                    generation: state.generation,
                    result: Mutex::new(None),
                    completed: Condvar::new(),
                });
                state.flight = Some(flight.clone());
                (flight, true)
            }
        };
        if start {
            let worker = flight.clone();
            let spawned = std::thread::Builder::new()
                .name("tun-helper-status".to_string())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                        .unwrap_or_else(|_| Err("TUN Helper 状态检查异常结束".to_string()));
                    if let Ok(mut slot) = worker.result.lock() {
                        *slot = Some((Instant::now(), result));
                        worker.completed.notify_all();
                    }
                });
            if let Err(error) = spawned {
                if let Ok(mut slot) = flight.result.lock() {
                    *slot = Some((
                        Instant::now(),
                        Err(format!("创建 Helper 状态检查失败：{error}")),
                    ));
                    flight.completed.notify_all();
                }
            }
        }

        let Ok(mut result) = flight.result.lock() else {
            return ProbeRead::Complete(Err("TUN Helper 状态结果锁损坏".to_string()));
        };
        let outcome = loop {
            if let Some((_, result)) = result.as_ref() {
                break ProbeRead::Complete(result.clone());
            }
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                // A reader timing out does not prove that the shared request or
                // launchd registration failed. Keep the single request alive.
                break ProbeRead::Checking;
            };
            match flight.completed.wait_timeout(result, remaining) {
                Ok((next, _)) => result = next,
                Err(_) => return ProbeRead::Complete(Err("TUN Helper 状态等待锁损坏".to_string())),
            }
        };
        drop(result);
        // Registration/repair invalidates old results, including late replies.
        match self.state.lock() {
            Ok(state) if state.generation == flight.generation => outcome,
            _ => ProbeRead::Checking,
        }
    }
}

#[derive(Default)]
pub(crate) struct HeartbeatState {
    generation: u64,
    pub(crate) error: Option<String>,
}

impl HeartbeatState {
    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
    }

    #[cfg(any(not(windows), test))]
    pub(crate) fn begin(&mut self) -> u64 {
        self.invalidate();
        self.generation
    }

    #[cfg(any(not(windows), test))]
    pub(crate) fn fail_if_current(&mut self, generation: u64, error: String) -> bool {
        if self.generation != generation {
            return false;
        }
        self.error = Some(error);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    const WAIT: Duration = Duration::from_secs(2);
    const CACHE: Duration = Duration::from_secs(10);

    #[test]
    fn concurrent_readers_share_one_request_and_result() {
        let probe = Arc::new(SharedProbe::<u32>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_probe = probe.clone();
        let first_calls = calls.clone();
        let first = std::thread::spawn(move || {
            first_probe.read(WAIT, CACHE, move || {
                first_calls.fetch_add(1, Ordering::SeqCst);
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(WAIT).unwrap();
                Ok(42)
            })
        });
        entered_rx.recv_timeout(WAIT).unwrap();
        let others: Vec<_> = (0..8)
            .map(|_| {
                let probe = probe.clone();
                let calls = calls.clone();
                std::thread::spawn(move || {
                    probe.read(WAIT, CACHE, move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(99)
                    })
                })
            })
            .collect();
        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap(), ProbeRead::Complete(Ok(42)));
        for other in others {
            assert_eq!(other.join().unwrap(), ProbeRead::Complete(Ok(42)));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn slow_probe_is_checking_then_late_reply_is_reused() {
        let probe = SharedProbe::<u32>::default();
        let (release_tx, release_rx) = mpsc::channel();
        assert_eq!(
            probe.read(Duration::from_millis(5), CACHE, move || {
                release_rx.recv_timeout(WAIT).unwrap();
                Ok(42)
            }),
            ProbeRead::Checking
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            probe.read(WAIT, CACHE, || panic!("must join existing request")),
            ProbeRead::Complete(Ok(42))
        );
    }

    #[test]
    fn actual_connection_error_is_shared_and_cache_expiry_retries() {
        let probe = SharedProbe::<u32>::default();
        let expected = ProbeRead::Complete(Err("connection rejected".to_string()));
        assert_eq!(
            probe.read(WAIT, CACHE, || Err("connection rejected".to_string())),
            expected
        );
        assert_eq!(
            probe.read(WAIT, CACHE, || panic!("must use shared error")),
            expected
        );
        assert_eq!(
            probe.read(WAIT, Duration::ZERO, || Ok(1)),
            ProbeRead::Complete(Ok(1))
        );
    }

    #[test]
    fn reset_fences_late_results_without_overwriting_new_probe() {
        let probe = Arc::new(SharedProbe::<u32>::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let old_probe = probe.clone();
        let old = std::thread::spawn(move || {
            old_probe.read(WAIT, CACHE, move || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(WAIT).unwrap();
                Ok(1)
            })
        });
        entered_rx.recv_timeout(WAIT).unwrap();
        probe.reset();
        assert_eq!(
            probe.read(WAIT, CACHE, || Ok(2)),
            ProbeRead::Complete(Ok(2))
        );
        release_tx.send(()).unwrap();
        assert_eq!(old.join().unwrap(), ProbeRead::Checking);
        assert_eq!(
            probe.read(WAIT, CACHE, || Ok(3)),
            ProbeRead::Complete(Ok(2))
        );
    }

    #[test]
    fn old_heartbeat_failure_cannot_publish_into_replacement_session() {
        let mut state = HeartbeatState::default();
        let old = state.begin();
        state.invalidate();
        let current = state.begin();
        assert!(!state.fail_if_current(old, "late old failure".to_string()));
        assert!(state.error.is_none());
        assert!(state.fail_if_current(current, "current failure".to_string()));
        assert_eq!(state.error.as_deref(), Some("current failure"));
    }

    #[test]
    fn stopped_heartbeat_cannot_publish_a_late_failure() {
        let mut state = HeartbeatState::default();
        let old = state.begin();
        state.invalidate();
        assert!(!state.fail_if_current(old, "late failure".to_string()));
        assert!(state.error.is_none());
    }
}
