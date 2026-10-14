//! Bounded install journal, independent of optional helper restoration.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub target: String,
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    pub stage: String,
    pub reason: String,
    pub disruption_started: bool,
}

fn valid(value: &Receipt) -> bool {
    matches!(value.schema, 1 | 2)
        && value.backup.as_ref().is_none_or(|s| s.len() <= 1024)
        && value.failure.as_ref().is_none_or(|f| {
            f.reason.len() <= 2048
                && matches!(
                    f.stage.as_str(),
                    "preflight" | "stopping" | "services" | "replacing" | "restarting"
                )
        })
        && value.target.len() <= 32
        && value.target.split('.').count() == 3
        && value
            .target
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && matches!(
            value.stage.as_str(),
            "preflight"
                | "stopping"
                | "services"
                | "replacing"
                | "restarting"
                | "failed"
                | "confirmed"
        )
}
fn read(path: &Path) -> AppResult<Option<Receipt>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.len() <= 4096 => {}
        _ => {
            return Err(AppError::Update(
                "更新记录格式异常，请保留记录并查看诊断".into(),
            ))
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(AppError::Update("更新记录超出大小限制".into()));
    }
    let value: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| AppError::Update("更新记录读取失败".into()))?;
    if !valid(&value) {
        return Err(AppError::Update("更新记录版本或状态异常".into()));
    }
    Ok(Some(value))
}
fn path(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Io(e.to_string()))?
        .join("app-update-receipt-v2.json"))
}
fn save(path: &Path, value: &Receipt) -> AppResult<()> {
    if !valid(value) {
        return Err(AppError::Update("更新记录参数异常".into()));
    }
    crate::storage::write_private_atomic(
        path,
        &serde_json::to_vec(value).map_err(|e| AppError::Update(e.to_string()))?,
    )
}

pub fn write(app: &AppHandle, target: &str, stage: &str) -> AppResult<()> {
    let path = path(app)?;
    let previous = read(&path)?;
    let backup = previous
        .filter(|r| r.target == target && stage != "preflight")
        .and_then(|r| r.backup);
    save(
        &path,
        &Receipt {
            schema: 2,
            target: target.into(),
            stage: stage.into(),
            failure: None,
            backup,
        },
    )
}

#[cfg(target_os = "macos")]
pub fn set_backup(app: &AppHandle, backup: &Path) -> AppResult<()> {
    let path = path(app)?;
    let mut receipt = read(&path)?.ok_or_else(|| AppError::Update("缺少安装预检记录".into()))?;
    receipt.backup = Some(backup.to_string_lossy().into_owned());
    save(&path, &receipt)
}

pub fn failed(
    app: &AppHandle,
    target: &str,
    stage: &str,
    reason: &str,
    disruption_started: bool,
) -> AppResult<()> {
    let path = path(app)?;
    let previous = read(&path)?;
    let backup = previous
        .filter(|r| r.target == target)
        .and_then(|r| r.backup);
    save(
        &path,
        &Receipt {
            schema: 2,
            target: target.into(),
            stage: "failed".into(),
            backup,
            failure: Some(Failure {
                stage: stage.into(),
                reason: crate::app_log::sanitize(reason).chars().take(400).collect(),
                disruption_started,
            }),
        },
    )
}

pub fn summary(app: &AppHandle) -> AppResult<Option<String>> {
    let path = path(app)?;
    let receipt = match read(&path)? {
        Some(r) => Some(r),
        None => read(&path.with_file_name("app-update-receipt-v1.json"))?,
    };
    let Some(mut receipt) = receipt else {
        return Ok(None);
    };
    let installed = receipt.target == env!("CARGO_PKG_VERSION");
    if installed {
        #[cfg(target_os = "macos")]
        {
            let root = crate::tun_service::codesign::bundle_root().map_err(AppError::Update)?;
            crate::tun_service::codesign::validate_bundle(&root, &receipt.target)
                .map_err(AppError::Update)?;
            if let Some(backup) = &receipt.backup {
                if let Err(error) = crate::macos_update_install::confirm_backup(
                    Path::new(backup),
                    &root,
                    &receipt.target,
                ) {
                    return Ok(Some(format!(
                        "已启动 v{}；备份清理未完成，资料仍保留：{}",
                        receipt.target,
                        crate::app_log::sanitize(&error.to_string())
                    )));
                }
            }
        }
        receipt.stage = "confirmed".into();
        receipt.schema = 2;
        receipt.backup = None;
        receipt.failure = None;
        save(&path, &receipt)?;
        return Ok(Some(format!(
            "已确认启动 v{}；辅助服务状态单独显示。",
            receipt.target
        )));
    }
    let detail = receipt
        .failure
        .as_ref()
        .map(|f| {
            format!(
                "{}；{}；{}",
                stage_label(&f.stage),
                if f.disruption_started {
                    "已进入停止/替换阶段，请核对代理与恢复结果"
                } else {
                    "未停止代理"
                },
                f.reason
            )
        })
        .unwrap_or_else(|| stage_label(&receipt.stage).into());
    let backup = receipt
        .backup
        .map(|p| format!(" 备份位置：{p}。"))
        .unwrap_or_default();
    Ok(Some(format!(
        "上次目标 v{} 尚未确认（{}）。当前为 v{}。{}",
        receipt.target,
        detail,
        env!("CARGO_PKG_VERSION"),
        backup
    )))
}
pub fn stage_label(stage: &str) -> &str {
    match stage {
        "preflight" => "安装预检",
        "stopping" => "停止旧进程",
        "services" => "准备服务",
        "replacing" => "替换应用",
        "restarting" => "等待重启",
        "failed" => "安装未完成",
        "confirmed" => "启动已确认",
        _ => "尚未开始",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_records_and_preflight_failures_remain_readable_and_bounded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("receipt");
        fs::write(&p, r#"{"schema":1,"target":"0.7.24","stage":"restarting"}"#).unwrap();
        assert!(read(&p).unwrap().unwrap().failure.is_none());
        let r = Receipt {
            schema: 2,
            target: "0.7.26".into(),
            stage: "failed".into(),
            backup: None,
            failure: Some(Failure {
                stage: "preflight".into(),
                reason: "磁盘空间不足".into(),
                disruption_started: false,
            }),
        };
        save(&p, &r).unwrap();
        let r = read(&p).unwrap().unwrap();
        assert!(!r.failure.unwrap().disruption_started);
        let r = Receipt {
            schema: 2,
            target: "0.7.26".into(),
            stage: "preflight".into(),
            backup: Some("x".repeat(1025)),
            failure: None,
        };
        assert!(save(&p, &r).is_err());
        assert_eq!(
            read(&p).unwrap().unwrap().stage,
            "failed",
            "invalid write preserves evidence"
        );
    }
    #[test]
    fn bounded_journal_rejects_invalid_states_and_links() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("receipt");
        assert!(read(&p).unwrap().is_none());
        fs::write(&p, r#"{"schema":1,"target":"0.7.24","stage":"replacing"}"#).unwrap();
        assert_eq!(read(&p).unwrap().unwrap().target, "0.7.24");
        fs::write(&p, r#"{"schema":1,"target":"../bad","stage":"replacing"}"#).unwrap();
        assert!(read(&p).is_err());
        fs::write(&p, vec![b' '; 4097]).unwrap();
        assert!(read(&p).is_err());
        #[cfg(unix)]
        {
            let link = d.path().join("link");
            std::os::unix::fs::symlink(&p, &link).unwrap();
            assert!(read(&link).is_err());
        }
    }
}
