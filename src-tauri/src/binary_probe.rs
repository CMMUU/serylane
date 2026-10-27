//! Explicit version probes only. Status snapshots never execute this operation.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
    #[cfg(windows)]
    file_id: (u32, u32, u32),
}

impl Identity {
    fn read(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        let meta = fs::metadata(&path)?;
        if !meta.is_file() {
            return Err(io::Error::other("核心路径不是文件"));
        }
        #[cfg(windows)]
        let file_id = windows_file_id(&path)?;
        Ok(Self {
            path,
            length: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            #[cfg(unix)]
            inode: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino())
            },
            #[cfg(windows)]
            file_id,
        })
    }
}

#[cfg(windows)]
fn windows_file_id(path: &Path) -> io::Result<(u32, u32, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let file = fs::File::open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live read-only handle; info is a writable structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

struct Entry {
    identity: Identity,
    result: Result<String, String>,
    at: Instant,
}
#[derive(Default)]
pub struct ProbeCache {
    entry: Mutex<Option<Entry>>,
    flight: Mutex<()>,
}

impl ProbeCache {
    pub fn version(&self, path: &Path) -> Option<String> {
        let identity = Identity::read(path).ok()?;
        self.entry
            .lock()
            .ok()?
            .as_ref()
            .filter(|entry| entry.identity == identity)?
            .result
            .as_ref()
            .ok()
            .cloned()
    }

    pub fn probe(
        &self,
        path: &Path,
        operation: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        let gate = &crate::lifecycle::PROCESS_LIFECYCLE;
        let ticket = gate.ticket().map_err(|e| e.to_string())?;
        let started = Instant::now();
        // Single-flight is bounded. Keep the cache mutex free while the process
        // runs, so status rendering never waits behind a slow/failed probe.
        let _flight = loop {
            gate.check(ticket).map_err(|e| e.to_string())?;
            match self.flight.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(_)) => return Err("核心探测状态不可用".into()),
                Err(TryLockError::WouldBlock) if started.elapsed() < Duration::from_secs(6) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err("核心版本检查繁忙，请稍后重试".into()),
            }
        };
        gate.check(ticket).map_err(|e| e.to_string())?;
        let identity = Identity::read(path).map_err(|e| e.to_string())?;
        if let Some(entry) = self
            .entry
            .lock()
            .map_err(|_| "核心探测缓存不可用")?
            .as_ref()
        {
            if entry.identity == identity
                && (entry.result.is_ok() || entry.at.elapsed() < Duration::from_secs(2))
            {
                return entry.result.clone();
            }
        }
        let result = operation();
        gate.check(ticket).map_err(|e| e.to_string())?;
        if Identity::read(path).ok().as_ref() != Some(&identity) {
            return Err("核心文件在检查期间发生变化，请重新检查".into());
        }
        *self.entry.lock().map_err(|_| "核心探测缓存不可用")? = Some(Entry {
            identity,
            result: result.clone(),
            at: Instant::now(),
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[test]
    fn repeated_and_concurrent_probes_execute_once_and_replacement_invalidates() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), b"v1").unwrap();
        let cache = Arc::new(ProbeCache::default());
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(cache.version(file.path()), None);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let (cache, calls, path) = (cache.clone(), calls.clone(), file.path());
                scope.spawn(move || {
                    assert_eq!(
                        cache
                            .probe(path, || {
                                calls.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(20));
                                Ok("v1".into())
                            })
                            .unwrap(),
                        "v1"
                    )
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.version(file.path()).as_deref(), Some("v1"));
        fs::write(file.path(), b"new version").unwrap();
        assert_eq!(cache.version(file.path()), None);
        assert_eq!(cache.probe(file.path(), || Ok("v2".into())).unwrap(), "v2");
    }
    #[test]
    fn snapshot_does_not_wait_for_probe_and_failures_are_temporarily_cached() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let cache = ProbeCache::default();
        let held = cache.flight.lock().unwrap();
        assert_eq!(cache.version(file.path()), None);
        drop(held);
        assert!(cache
            .probe(file.path(), || Err("probe failed".into()))
            .is_err());
        assert_eq!(cache.version(file.path()), None);
        assert_eq!(
            cache
                .probe(file.path(), || panic!("negative cache missed"))
                .unwrap_err(),
            "probe failed"
        );
        cache.entry.lock().unwrap().as_mut().unwrap().at = Instant::now() - Duration::from_secs(3);
        assert_eq!(
            cache.probe(file.path(), || Ok("recovered".into())).unwrap(),
            "recovered"
        );
    }
    #[test]
    fn replacement_during_probe_is_not_cached() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let cache = ProbeCache::default();
        assert!(cache
            .probe(file.path(), || {
                fs::write(file.path(), b"replacement").unwrap();
                Ok("stale".into())
            })
            .is_err());
        assert_eq!(cache.version(file.path()), None);
    }
}
