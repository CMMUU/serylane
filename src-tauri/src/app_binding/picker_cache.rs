//! A single background refresh preserves a usable previous snapshot. The UI
//! polls this cheap state, never a native enumeration or a configuration lock.
use super::CatalogSnapshot;
use serde::Serialize;
use std::time::{Duration, Instant};
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickerSnapshot {
    #[serde(flatten)]
    pub catalog: CatalogSnapshot,
    pub refreshing: bool,
    pub checked_at: Option<i64>,
}
#[derive(Default)]
pub struct PickerCache {
    pub snapshot: PickerSnapshot,
    checked: Option<Instant>,
}
impl PickerCache {
    pub fn begin(&mut self, force: bool) -> bool {
        if self.snapshot.refreshing
            || (!force
                && self
                    .checked
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(30)))
        {
            return false;
        }
        self.snapshot.refreshing = true;
        true
    }
    pub fn finish(&mut self, result: Result<CatalogSnapshot, String>) {
        match result {
            Ok(catalog) => self.snapshot.catalog = catalog,
            Err(message) => self.snapshot.catalog.warnings = vec![message],
        }
        self.snapshot.refreshing = false;
        self.snapshot.checked_at = Some(chrono::Utc::now().timestamp_millis());
        self.checked = Some(Instant::now());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refreshes_are_single_flight_and_failure_retains_previous_snapshot() {
        let mut cache = PickerCache::default();
        assert!(cache.begin(false));
        assert!(!cache.begin(true));
        let binding = super::super::WindowsBinding {
            package_family_name: "Example.App_123456789abcd".into(),
            application_id: "App".into(),
        };
        cache.finish(Ok(CatalogSnapshot {
            applications: vec![super::super::InstalledApplication {
                binding: binding.into(),
                name: "cached".into(),
                version: "1".into(),
                package_full_name: String::new(),
                package_root: String::new(),
                executable: String::new(),
                availability: super::super::AppAvailability::Ready,
                detail: String::new(),
            }],
            warnings: vec!["old warning".into()],
            ..Default::default()
        }));
        assert!(!cache.begin(false));
        assert!(cache.begin(true));
        cache.finish(Err("读取失败，已保留旧清单".into()));
        assert_eq!(cache.snapshot.catalog.warnings, ["读取失败，已保留旧清单"]);
        assert_eq!(cache.snapshot.catalog.applications[0].name, "cached");
        assert!(!cache.snapshot.refreshing);
        assert!(cache.snapshot.checked_at.is_some());
    }
}
