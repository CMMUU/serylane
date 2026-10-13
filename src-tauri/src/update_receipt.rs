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
}

fn valid(value: &Receipt) -> bool {
    value.schema == 1
        && value.target.len() <= 32
        && value.target.split('.').count() == 3
        && value
            .target
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        && matches!(
            value.stage.as_str(),
            "stopping" | "services" | "replacing" | "restarting" | "failed" | "confirmed"
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
        .join("app-update-receipt-v1.json"))
}
pub fn write(app: &AppHandle, target: &str, stage: &str) -> AppResult<()> {
    let path = path(app)?;
    read(&path)?; // Reject malformed or linked records rather than overwriting them.
    let value = Receipt {
        schema: 1,
        target: target.into(),
        stage: stage.into(),
    };
    if !valid(&value) {
        return Err(AppError::Update("更新记录参数异常".into()));
    }
    crate::storage::write_private_atomic(
        &path,
        &serde_json::to_vec(&value).map_err(|e| AppError::Update(e.to_string()))?,
    )
}
pub fn summary(app: &AppHandle) -> AppResult<Option<String>> {
    let Some(receipt) = read(&path(app)?)? else {
        return Ok(None);
    };
    let installed = receipt.target == env!("CARGO_PKG_VERSION");
    if installed && receipt.stage != "confirmed" {
        write(app, &receipt.target, "confirmed")?;
    }
    Ok(Some(if installed {
        format!("已确认启动 v{}；辅助服务状态单独显示。", receipt.target)
    } else {
        format!(
            "上次目标 v{} 尚未在本次启动确认（阶段：{}）。当前仍为 v{}，请重新检查更新。",
            receipt.target,
            stage_label(&receipt.stage),
            env!("CARGO_PKG_VERSION")
        )
    }))
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
