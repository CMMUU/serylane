//! Platform-tagged desktop identities. Legacy Windows bindings keep their wire
//! shape so migration does not reinterpret an existing user's application.
use super::{AppAvailability, AppBinding, InstalledApplication};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DesktopBinding {
    Macos {
        bundle_id: String,
        location: String,
        requirement: Option<String>,
    },
    Linux {
        desktop_id: String,
        location: String,
    },
}
impl DesktopBinding {
    pub fn valid(&self) -> bool {
        let clean = |s: &str| !s.is_empty() && s.len() <= 32760 && !s.contains(['\0', '\r', '\n']);
        match self {
            Self::Macos {
                bundle_id,
                location,
                requirement,
            } => {
                clean(bundle_id)
                    && clean(location)
                    && location.starts_with('/')
                    && location.to_ascii_lowercase().ends_with(".app")
                    && requirement
                        .as_ref()
                        .is_none_or(|r| clean(r) && r.len() <= 8192)
            }
            Self::Linux {
                desktop_id,
                location,
            } => {
                desktop_id.len() <= 1024
                    && !desktop_id.contains(['/', '\\', '\0', '\r', '\n'])
                    && (desktop_id.is_empty() || desktop_id.ends_with(".desktop"))
                    && clean(location)
                    && location.starts_with('/')
                    && location.ends_with(".desktop")
            }
        }
    }
    pub fn platform(&self) -> &'static str {
        match self {
            Self::Macos { .. } => "macos",
            Self::Linux { .. } => "linux",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DesktopApplication {
    pub binding: DesktopBinding,
    pub name: String,
    pub version: String,
    pub root: String,
    pub executable: String,
    pub availability: AppAvailability,
    pub detail: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
}
impl From<DesktopApplication> for InstalledApplication {
    fn from(app: DesktopApplication) -> Self {
        Self {
            binding: AppBinding::Desktop(app.binding),
            name: app.name,
            version: app.version,
            package_full_name: String::new(),
            package_root: app.root,
            executable: app.executable,
            availability: app.availability,
            detail: app.detail,
        }
    }
}

pub fn collect() -> Result<Vec<DesktopApplication>, String> {
    #[cfg(target_os = "macos")]
    {
        super::macos::collect()
    }
    #[cfg(target_os = "linux")]
    {
        super::linux::collect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("此平台请使用 Windows 应用身份或普通文件。".into())
    }
}
pub fn resolve(binding: &DesktopBinding) -> Result<DesktopApplication, String> {
    if !binding.valid() || binding.platform() != std::env::consts::OS {
        return Err("此应用属于其他平台或身份信息已变化，请重新关联。".into());
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::resolve(binding)
    }
    #[cfg(target_os = "linux")]
    {
        super::linux::resolve(binding)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("此应用属于其他平台，请重新选择本机应用。".into())
    }
}
pub fn inspect(path: &Path) -> Result<DesktopApplication, String> {
    if !path.is_absolute() {
        return Err("请选择本机应用的绝对路径。".into());
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::inspect(path)
    }
    #[cfg(target_os = "linux")]
    {
        super::linux::inspect(path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("请使用 Windows 已安装应用选择器。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_identity_is_tagged_and_strict() {
        let app = DesktopBinding::Linux {
            desktop_id: String::new(),
            location: "/opt/My App.desktop".into(),
        };
        assert!(app.valid());
        let wire = serde_json::to_string(&app).unwrap();
        assert!(wire.contains("desktopId"));
        assert_eq!(serde_json::from_str::<DesktopBinding>(&wire).unwrap(), app);
        assert!(serde_json::from_str::<DesktopBinding>(
            r#"{"kind":"linux","location":"/a.desktop","desktopId":"a.desktop","extra":1}"#
        )
        .is_err());
        assert!(!DesktopBinding::Linux {
            desktop_id: "../a.desktop".into(),
            location: "/a.desktop".into()
        }
        .valid());
        assert!(!DesktopBinding::Macos {
            bundle_id: "app.example".into(),
            location: "relative.app".into(),
            requirement: None
        }
        .valid());
    }
}
