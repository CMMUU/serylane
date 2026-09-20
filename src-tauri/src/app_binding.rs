//! Version-independent application identity. Resolved paths are runtime data,
//! never the identity of a packaged application.
use serde::{Deserialize, Serialize};
#[path = "app_binding/desktop.rs"]
pub mod desktop;
#[cfg(any(target_os = "linux", all(test, unix)))]
#[path = "app_binding/linux.rs"]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod linux;
#[cfg(target_os = "macos")]
#[path = "app_binding/macos.rs"]
pub mod macos;
#[path = "app_binding/picker_cache.rs"]
pub mod picker_cache;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AppBinding {
    Windows(WindowsBinding),
    Desktop(desktop::DesktopBinding),
}
impl From<WindowsBinding> for AppBinding {
    fn from(value: WindowsBinding) -> Self {
        Self::Windows(value)
    }
}
impl AppBinding {
    pub fn windows(&self) -> Option<&WindowsBinding> {
        match self {
            Self::Windows(w) => Some(w),
            _ => None,
        }
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Windows(w) => w.valid(),
            Self::Desktop(d) => d.valid(),
        }
    }
    pub fn platform(&self) -> &'static str {
        match self {
            Self::Windows(_) => "windows",
            Self::Desktop(d) => d.platform(),
        }
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn aumid(&self) -> String {
        match self {
            Self::Windows(w) => w.aumid(),
            Self::Desktop(d) => serde_json::to_string(d).unwrap_or_default(),
        }
    }
}

#[cfg(windows)]
#[path = "app_binding/windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::running_bound;
#[cfg(windows)]
pub use windows::ApplicationCatalog;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowsBinding {
    pub package_family_name: String,
    pub application_id: String,
}

impl WindowsBinding {
    pub fn valid(&self) -> bool {
        let Some((name, publisher)) = self.package_family_name.rsplit_once('_') else {
            return false;
        };
        (3..=50).contains(&name.len())
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
            && publisher.len() == 13
            && publisher.bytes().all(|c| c.is_ascii_alphanumeric())
            && !self.application_id.is_empty()
            && self.application_id.len() <= 64
            && self
                .application_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.')
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn aumid(&self) -> String {
        format!("{}!{}", self.package_family_name, self.application_id)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppAvailability {
    Ready,
    NotInstalled,
    Updating,
    Maintenance,
    NeedsRelink,
    MissingFile,
    UnsupportedLaunch,
    ReadError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApplication {
    pub binding: AppBinding,
    pub name: String,
    pub version: String,
    pub package_full_name: String,
    pub package_root: String,
    pub executable: String,
    pub availability: AppAvailability,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub applications: Vec<InstalledApplication>,
    // One record per registered MAIN package, including unreadable manifests.
    // This prevents a read error from masquerading as an uninstall.
    #[serde(skip)]
    pub packages: Vec<PackageRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub family: String,
    pub full_name: String,
    pub availability: AppAvailability,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationResolution {
    pub availability: AppAvailability,
    pub detail: String,
    pub application: Option<InstalledApplication>,
}

impl ApplicationResolution {
    pub fn state(availability: AppAvailability, detail: impl Into<String>) -> Self {
        Self {
            availability,
            detail: detail.into(),
            application: None,
        }
    }
}

pub fn resolve(binding: &AppBinding, catalog: &CatalogSnapshot) -> ApplicationResolution {
    use AppAvailability::*;
    let Some(binding) = binding.windows() else {
        return ApplicationResolution::state(UnsupportedLaunch, "请使用对应平台的应用入口。");
    };
    let packages: Vec<_> = catalog
        .packages
        .iter()
        .filter(|p| p.family.eq_ignore_ascii_case(&binding.package_family_name))
        .collect();
    if packages.is_empty() {
        return if catalog.warnings.is_empty() {
            ApplicationResolution::state(
                NotInstalled,
                "当前 Windows 账户尚未安装此应用，原代理配置已保留。",
            )
        } else {
            ApplicationResolution::state(ReadError, "应用注册信息读取不完整，请点击刷新重试。")
        };
    }
    if let Some(package) = packages
        .iter()
        .find(|p| matches!(p.availability, Updating | Maintenance))
    {
        return ApplicationResolution::state(package.availability, &package.detail);
    }
    if packages.len() != 1 {
        return ApplicationResolution::state(
            NeedsRelink,
            "发现多个注册安装，暂未选择启动版本。请等待更新完成后刷新，或重新关联。",
        );
    }
    let package = packages[0];
    if package.availability != Ready {
        return ApplicationResolution::state(package.availability, &package.detail);
    }
    let apps: Vec<_> = catalog
        .applications
        .iter()
        .filter(|app| {
            app.package_full_name
                .eq_ignore_ascii_case(&package.full_name)
                && app
                    .binding
                    .windows()
                    .is_some_and(|w| w.application_id == binding.application_id)
        })
        .collect();
    if apps.len() != 1 {
        return ApplicationResolution::state(
            NeedsRelink,
            "应用启动入口已变化，请重新选择应用；原代理设置和参数已保留。",
        );
    }
    let application = apps[0].clone();
    ApplicationResolution {
        availability: application.availability,
        detail: application.detail.clone(),
        application: Some(application),
    }
}

pub fn relative_windows_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32760
        && !value.contains([':', '\0', '\r', '\n', '*', '?', '"', '<', '>', '|'])
        && !value.starts_with(['\\', '/'])
        && value
            .split(['\\', '/'])
            .all(|s| !s.is_empty() && s != "." && s != ".." && !s.ends_with([' ', '.']))
}

pub fn path_key(value: &str) -> String {
    value
        .trim_start_matches("\\\\?\\")
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

pub fn install_relative(root: &str, path: &str) -> Option<String> {
    let root_key = format!("{}\\", path_key(root));
    let path = path.trim_start_matches("\\\\?\\").replace('/', "\\");
    if path_key(&path).starts_with(&root_key) {
        let root_length = root
            .trim_start_matches("\\\\?\\")
            .replace('/', "\\")
            .trim_end_matches('\\')
            .len()
            + 1;
        let suffix = path.get(root_length..)?;
        relative_windows_path(suffix).then(|| suffix.to_string())
    } else {
        None
    }
}

/// Migrate only an exact, unique, registered executable. No folder-name or
/// display-name guessing, including when the old version has already vanished.
pub fn exact_legacy_match<'a>(
    executable: &str,
    catalog: &'a CatalogSnapshot,
) -> Option<&'a InstalledApplication> {
    if !catalog.warnings.is_empty() {
        return None;
    }
    let matches: Vec<_> = catalog
        .applications
        .iter()
        .filter(|app| {
            app.availability == AppAvailability::Ready
                && path_key(&app.executable) == path_key(executable)
                && resolve(&app.binding, catalog).availability == AppAvailability::Ready
        })
        .collect();
    (matches.len() == 1).then(|| matches[0])
}

#[cfg(not(windows))]
#[derive(Default)]
pub struct ApplicationCatalog;
#[cfg(not(windows))]
impl ApplicationCatalog {
    pub fn query(&self, _family: Option<&str>, _fresh: bool) -> Result<CatalogSnapshot, String> {
        Ok(CatalogSnapshot {
            applications: desktop::collect()?.into_iter().map(Into::into).collect(),
            packages: vec![],
            warnings: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> AppBinding {
        WindowsBinding {
            package_family_name: "Example.App_123456789abcd".into(),
            application_id: "App".into(),
        }
        .into()
    }
    fn catalog(version: &str) -> CatalogSnapshot {
        let full = format!("Example.App_{version}_x64__123456789abcd");
        let root = format!("C:\\Program Files\\WindowsApps\\{full}");
        CatalogSnapshot {
            applications: vec![InstalledApplication {
                binding: binding(),
                name: "Example".into(),
                version: version.into(),
                package_full_name: full.clone(),
                package_root: root.clone(),
                executable: format!("{root}\\app\\Example.exe"),
                availability: AppAvailability::Ready,
                detail: "已关联".into(),
            }],
            packages: vec![PackageRecord {
                family: binding().windows().unwrap().package_family_name.clone(),
                full_name: full,
                availability: AppAvailability::Ready,
                detail: String::new(),
            }],
            warnings: vec![],
        }
    }
    #[test]
    fn upgrade_resolves_same_identity_without_persisting_version() {
        let id = binding();
        let old = resolve(&id, &catalog("1.0.0.0")).application.unwrap();
        let new = resolve(&id, &catalog("2.0.0.0")).application.unwrap();
        assert_ne!(old.executable, new.executable);
        assert_eq!(old.binding, new.binding);
        assert!(!serde_json::to_string(&id).unwrap().contains("1.0.0.0"));
    }
    #[test]
    fn missing_ambiguous_and_changed_identity_do_not_guess() {
        assert_eq!(
            resolve(&binding(), &CatalogSnapshot::default()).availability,
            AppAvailability::NotInstalled
        );
        let mut c = catalog("1.0.0.0");
        c.packages.extend(catalog("2.0.0.0").packages);
        assert_eq!(
            resolve(&binding(), &c).availability,
            AppAvailability::NeedsRelink
        );
        let mut c = catalog("1.0.0.0");
        if let AppBinding::Windows(w) = &mut c.applications[0].binding {
            w.application_id = "Other".into();
        }
        assert_eq!(
            resolve(&binding(), &c).availability,
            AppAvailability::NeedsRelink
        );
        c.packages[0].availability = AppAvailability::ReadError;
        assert_eq!(
            resolve(&binding(), &c).availability,
            AppAvailability::ReadError
        );
    }
    #[test]
    fn update_requires_system_evidence_and_read_errors_are_not_uninstalls() {
        let mut c = catalog("1.0.0.0");
        c.packages[0].availability = AppAvailability::Updating;
        assert_eq!(
            resolve(&binding(), &c).availability,
            AppAvailability::Updating
        );
        c = CatalogSnapshot::default();
        c.warnings.push("query failed".into());
        assert_eq!(
            resolve(&binding(), &c).availability,
            AppAvailability::ReadError
        );
    }
    #[test]
    fn migration_requires_unique_exact_executable() {
        let mut c = catalog("2.0.0.0");
        assert!(exact_legacy_match(&c.applications[0].executable, &c).is_some());
        assert!(exact_legacy_match(&catalog("1.0.0.0").applications[0].executable, &c).is_none());
        c.applications.push(c.applications[0].clone());
        assert!(exact_legacy_match(&c.applications[0].executable, &c).is_none());
    }
    #[test]
    fn relative_directories_preserve_boundaries_and_reject_traversal() {
        assert_eq!(
            install_relative("C:\\Apps\\One", "c:\\apps\\one\\work"),
            Some("work".into())
        );
        assert!(install_relative("C:\\Apps\\One", "C:\\Apps\\OneOther\\work").is_none());
        for path in [
            "..\\x",
            "x\\..\\y",
            "C:\\x",
            "\\\\server\\x",
            "x:y",
            "x\\",
            "x.\\y",
        ] {
            assert!(!relative_windows_path(path));
        }
        assert!(relative_windows_path("app\\工作目录"));
        assert!(binding().valid());
    }
}
