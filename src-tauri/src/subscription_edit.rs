//! Metadata-only editing: source changes are downloaded by an explicit refresh.
use crate::{
    error::{AppError, AppErrorDto, AppResult},
    models::{ProfileRecord, ProfileSource},
    storage::AppStorage,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use uuid::Uuid;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    pub profile_id: Uuid,
    pub expected_revision: String,
    pub display_name: String,
    pub url: String,
    pub user_agent: String,
}
fn revision(profile: &ProfileRecord) -> AppResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(profile).map_err(|_| AppError::Config("读取订阅修订失败".into()))?
        )
    ))
}
fn read(storage: &AppStorage, id: Uuid) -> AppResult<Edit> {
    let profile = storage.load_profile(id)?;
    let ProfileSource::RemoteSubscription { url, user_agent } = &profile.source else {
        return Err(AppError::InvalidInput("仅远程订阅支持此编辑入口".into()));
    };
    Ok(Edit {
        profile_id: id,
        expected_revision: revision(&profile)?,
        display_name: profile.display_name.clone(),
        url: url.clone(),
        user_agent: user_agent.clone(),
    })
}
fn save(storage: &AppStorage, input: Edit) -> AppResult<Edit> {
    let mut profile = storage.load_profile(input.profile_id)?;
    if revision(&profile)? != input.expected_revision {
        return Err(AppError::Conflict(
            "订阅已变化，请重新读取；未覆盖更改".into(),
        ));
    }
    let ProfileSource::RemoteSubscription {
        url: old_url,
        user_agent: old_ua,
    } = &profile.source
    else {
        return Err(AppError::InvalidInput("仅远程订阅支持编辑".into()));
    };
    let name = input.display_name.trim();
    let url = input.url.trim();
    let ua = input.user_agent.trim();
    if name.is_empty() || name.len() > 200 || name.chars().any(char::is_control) {
        return Err(AppError::InvalidInput(
            "订阅名称需为 1～200 字节且不含控制字符".into(),
        ));
    }
    if url.len() > 8192 || ua.len() > 512 || ua.chars().any(char::is_control) {
        return Err(AppError::InvalidInput(
            "订阅地址或客户端标识过长或包含无效字符".into(),
        ));
    }
    let canonical = crate::subscription::validate_subscription_url(url)?.to_string();
    for other in storage.list_profiles()? {
        if other.id == profile.id {
            continue;
        }
        if let ProfileSource::RemoteSubscription { url, .. } = &other.source {
            if crate::subscription::validate_subscription_url(url)
                .is_ok_and(|u| u.as_str() == canonical)
            {
                return Err(AppError::Conflict(
                    "此订阅地址已存在，请编辑对应订阅；未合并或删除条目".into(),
                ));
            }
        }
    }
    let ua = if ua.is_empty() { "clash.meta" } else { ua };
    if old_url != &canonical || old_ua != ua {
        profile.subscription_source_updated_at = Some(chrono::Utc::now());
    }
    profile.display_name = name.to_string();
    profile.source = ProfileSource::RemoteSubscription {
        url: canonical,
        user_agent: ua.to_string(),
    };
    profile.updated_at = chrono::Utc::now();
    storage.save_profile(&profile)?;
    read(storage, profile.id)
}
#[tauri::command]
pub fn read_subscription_edit(app: AppHandle, profile_id: Uuid) -> Result<Edit, AppErrorDto> {
    AppStorage::from_app(&app)
        .and_then(|s| read(&s, profile_id))
        .map_err(|e| e.dto())
}
#[tauri::command]
pub fn save_subscription_edit(
    app: AppHandle,
    input: Edit,
    confirmed: bool,
) -> Result<Edit, AppErrorDto> {
    (|| -> AppResult<_> {
        if !confirmed {
            return Err(AppError::InvalidInput("修改订阅信息需要确认".into()));
        }
        let _permit = crate::user_rules::acquire_configuration(&app)?;
        save(&AppStorage::from_app(&app)?, input)
    })()
    .map_err(|e| e.dto())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_preserves_identity_and_rejects_stale_write() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().to_path_buf()).unwrap();
        let p = storage
            .create_profile(
                "before".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://example.invalid/a".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .unwrap();
        let mut edit = read(&storage, p.id).unwrap();
        let stale = read(&storage, p.id).unwrap();
        edit.display_name = "after".into();
        edit.url = "https://example.invalid/b".into();
        let saved = save(&storage, edit).unwrap();
        assert_eq!(saved.profile_id, p.id);
        let now = storage.load_profile(p.id).unwrap();
        assert_eq!(now.active_revision_id, p.active_revision_id);
        assert_eq!(now.created_at, p.created_at);
        assert!(now.subscription_source_updated_at.is_some());
        assert!(save(&storage, stale).is_err());
    }
    #[test]
    fn invalid_and_duplicate_edits_leave_original_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().to_path_buf()).unwrap();
        let profile = storage
            .create_profile(
                "original".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://example.invalid/one".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .unwrap();
        storage
            .create_profile(
                "other".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://example.invalid/two".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .unwrap();
        let before = read(&storage, profile.id).unwrap().expected_revision;
        for url in ["file:///tmp/fixture", "https://example.invalid/two"] {
            let mut edit = read(&storage, profile.id).unwrap();
            edit.url = url.into();
            assert!(save(&storage, edit).is_err());
            assert_eq!(
                read(&storage, profile.id).unwrap().expected_revision,
                before
            );
        }
        let mut edit = read(&storage, profile.id).unwrap();
        edit.display_name = "renamed".into();
        save(&storage, edit).unwrap();
        assert!(storage
            .load_profile(profile.id)
            .unwrap()
            .subscription_source_updated_at
            .is_none());
    }
}
