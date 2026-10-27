//! Process admission is separate from cleanup: querying shutdown must be fast
//! and reversible, while confirmed exit is terminal. Never hold this gate while
//! waiting for a child, doing network I/O or acquiring a runtime-transition lock.
use serde::Serialize;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

const OPEN: u64 = 0;
const PREPARING: u64 = 1;
const CLOSING: u64 = 2;
const UPDATING: u64 = 4;
const MASK: u64 = 7;
pub static PROCESS_LIFECYCLE: Lifecycle = Lifecycle::new();

pub struct Lifecycle {
    // Low bits are independent pause/exit reasons; upper bits are an epoch. Cancel does not revive
    // work queued in a previous epoch (including prepare/cancel/prepare ABA).
    state: AtomicU64,
    spawn: Mutex<()>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub generation: u64,
    pub phase: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub enum Purpose {
    Runtime,
    Validation,
    Version,
}

impl Purpose {
    pub fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Validation => "validation",
            Self::Version => "version",
        }
    }
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            state: AtomicU64::new(OPEN),
            spawn: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.load(Ordering::Acquire);
        Snapshot {
            generation: state >> 3,
            phase: if state & CLOSING != 0 {
                "closing"
            } else if state & PREPARING != 0 {
                "preparing"
            } else if state & UPDATING != 0 {
                "updating"
            } else {
                "open"
            },
        }
    }

    #[cfg(any(windows, test))]
    pub fn prepare(&self) -> bool {
        self.flag(PREPARING, true)
    }

    #[cfg(any(windows, test))]
    pub fn cancel(&self) -> bool {
        self.flag(PREPARING, false)
    }

    pub fn close(&self) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                (old & CLOSING == 0).then_some((((old >> 3) + 1) << 3) | CLOSING | (old & MASK))
            });
    }

    fn flag(&self, flag: u64, set: bool) -> bool {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                if old & CLOSING != 0 || (old & flag != 0) == set {
                    return None;
                }
                Some(
                    (((old >> 3) + 1) << 3)
                        | if set {
                            (old & MASK) | flag
                        } else {
                            (old & MASK) & !flag
                        },
                )
            })
            .is_ok()
    }

    pub fn pause_update(&self) -> io::Result<UpdatePause<'_>> {
        let old = self.ticket()?;
        self.state
            .compare_exchange(
                old,
                (((old >> 3) + 1) << 3) | UPDATING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| {
                io::Error::new(io::ErrorKind::Interrupted, "会话状态已变化，未开始安装")
            })?;
        Ok(UpdatePause(self))
    }

    pub fn ticket(&self) -> io::Result<u64> {
        let ticket = self.state.load(Ordering::Acquire);
        self.check(ticket)?;
        Ok(ticket)
    }

    pub fn check(&self, ticket: u64) -> io::Result<()> {
        if ticket & MASK != OPEN || self.state.load(Ordering::Acquire) != ticket {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "应用正在结束会话或会话状态已变化，请稍后重试",
            ));
        }
        Ok(())
    }

    pub fn admitting(&self) -> bool {
        self.ticket().is_ok()
    }

    pub fn admission(&self) -> io::Result<MutexGuard<'_, ()>> {
        let ticket = self.ticket()?;
        let start = Instant::now();
        loop {
            self.check(ticket)?;
            match self.spawn.try_lock() {
                Ok(guard) => {
                    self.check(ticket)?;
                    return Ok(guard);
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err(io::Error::other("进程创建门禁不可用"))
                }
                Err(TryLockError::WouldBlock) if start.elapsed() < Duration::from_millis(250) => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "进程创建繁忙，请稍后重试",
                    ))
                }
            }
        }
    }
}

pub struct UpdatePause<'a>(&'a Lifecycle);
impl Drop for UpdatePause<'_> {
    fn drop(&mut self) {
        self.0.flag(UPDATING, false);
    }
}

fn system_shutting_down() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
        // SAFETY: a read-only metric for this process's session.
        unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn admission(purpose: Purpose) -> io::Result<MutexGuard<'static, ()>> {
    let result = PROCESS_LIFECYCLE.admission().and_then(|guard| {
        if system_shutting_down() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "系统正在结束会话，已跳过核心进程创建",
            ));
        }
        Ok(guard)
    });
    if result.is_err() {
        crate::app_log::record(
            0,
            crate::app_log::Area::Runtime,
            &format!(
                "core_process purpose={} action=skip phase={}",
                purpose.label(),
                PROCESS_LIFECYCLE.snapshot().phase
            ),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_shutdown_restores_admission_but_not_old_work() {
        let gate = Lifecycle::new();
        let old = gate.ticket().unwrap();
        assert!(gate.prepare());
        assert!(!gate.prepare());
        assert!(gate.admission().is_err());
        assert!(gate.cancel());
        assert!(gate.admission().is_ok());
        assert!(gate.check(old).is_err());
        gate.close();
        assert!(!gate.cancel());
        assert!(!gate.prepare());
        assert!(gate.admission().is_err());
    }
    #[test]
    fn preparation_never_waits_for_an_already_admitted_spawn() {
        let gate = Lifecycle::new();
        let admitted = gate.admission().unwrap();
        assert!(gate.prepare());
        assert!(gate.admission().is_err());
        drop(admitted);
        assert!(gate.cancel());
        assert!(gate.admission().is_ok());
    }
    #[test]
    fn update_pause_and_shutdown_cancel_never_clear_each_other() {
        let gate = Lifecycle::new();
        let pause = gate.pause_update().unwrap();
        assert_eq!(gate.snapshot().phase, "updating");
        assert!(gate.prepare());
        assert!(gate.cancel());
        assert!(!gate.admitting());
        gate.prepare();
        drop(pause);
        assert_eq!(gate.snapshot().phase, "preparing");
        gate.cancel();
        assert!(gate.admitting());
        let pause = gate.pause_update().unwrap();
        gate.close();
        drop(pause);
        assert_eq!(gate.snapshot().phase, "closing");
    }
    #[test]
    fn repeated_close_is_idempotent_and_never_reopens() {
        let gate = Lifecycle::new();
        gate.close();
        let generation = gate.snapshot().generation;
        for _ in 0..100 {
            gate.close();
            assert!(!gate.cancel());
        }
        assert_eq!(gate.snapshot().generation, generation);
    }
    #[test]
    fn contended_admission_is_bounded_and_no_spawn_follows_close() {
        let gate = std::sync::Arc::new(Lifecycle::new());
        let held = gate.admission().unwrap();
        let other = gate.clone();
        let waiting = std::thread::spawn(move || other.admission().is_err());
        gate.close();
        assert!(waiting.join().unwrap());
        drop(held);
    }
}
