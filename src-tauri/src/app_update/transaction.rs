//! The product command and native acceptance tests execute this same coordinator.
//! Only the OS boundary is injected; candidate validation and state ordering are not.
use super::*;

pub(super) trait InstallHost {
    type Prepared;
    fn stage(&self, target: &str, stage: &str) -> AppResult<()>;
    fn preflight(&self, candidate: &Candidate) -> AppResult<Self::Prepared>;
    fn stop(&self) -> AppResult<()>;
    fn prepare_services(&self, target: &str) -> AppResult<()>;
    fn replace(
        &self,
        candidate: &Candidate,
        bytes: Vec<u8>,
        _prepared: Self::Prepared,
    ) -> AppResult<()>;
    fn rollback_services(&self) -> AppResult<()>;
    fn restart(&self) -> AppResult<()>;
    fn failed(&self, target: &str, stage: &str, reason: &str, disrupted: bool) -> AppResult<()>;
}

pub(super) fn run_install<H: InstallHost>(
    session: &Mutex<Session>,
    candidate: &Candidate,
    current: &str,
    platform: &UpdatePlatform,
    host: &H,
) -> AppResult<()> {
    let target = &candidate.update.version;
    let stage = |value: &str| -> AppResult<()> {
        // Persist intent before any disruption. A failed journal cannot permit installation.
        host.stage(target, value)?;
        session
            .lock()
            .map_err(|_| failure("更新状态锁不可用"))?
            .status
            .stage = value.into();
        Ok(())
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stage("preflight")?;
        {
            let mut state = session.lock().map_err(|_| failure("更新状态锁不可用"))?;
            let checked = state
                .ready
                .as_ref()
                .ok_or_else(|| failure("安装包已失效，请重新下载"))
                .and_then(|(ready, bytes)| {
                    ready.verify_for_install(candidate, bytes, current, platform)
                });
            if let Err(error) = checked {
                state.ready = None; // Invalid identity/bytes are never a retryable environment failure.
                return Err(error);
            }
        }
        let prepared = host.preflight(candidate)?;
        stage("stopping")?;
        let (_, bytes) = session
            .lock()
            .map_err(|_| failure("更新状态锁不可用"))?
            .ready
            .take()
            .ok_or_else(|| failure("安装包已失效"))?;
        host.stop()?;
        stage("services")?;
        install_with_service_transaction(
            || host.prepare_services(target),
            || {
                stage("replacing")?;
                host.replace(candidate, bytes, prepared)
            },
            || host.rollback_services(),
        )?;
        stage("restarting")?;
        host.restart()?;
        Ok(())
    }))
    .unwrap_or_else(|_| Err(failure("安装任务意外中断；请核对日志与保留的备份")));
    if let Err(error) = &result {
        let (failed_stage, retry) = {
            let mut state = session.lock().map_err(|_| failure("更新状态锁不可用"))?;
            let retry = state.ready.is_some() && state.status.stage == "preflight";
            state.status.phase = "failed".into();
            state.status.retry_install = retry;
            state.status.error = Some(crate::app_log::sanitize(&error.to_string()));
            (state.status.stage.clone(), retry)
        };
        crate::app_log::record(
            2,
            crate::app_log::Area::App,
            &format!("软件更新 v{target} 失败 [{failed_stage}，可重试安装={retry}]：{error}"),
        );
        if let Err(journal) = host.failed(
            target,
            &failed_stage,
            &error.to_string(),
            failed_stage != "preflight",
        ) {
            crate::app_log::record(
                1,
                crate::app_log::Area::App,
                &format!("更新失败记录写入未完成：{journal}"),
            );
        }
    }
    result
}

pub(super) struct NativeHost {
    pub app: AppHandle,
}
impl InstallHost for NativeHost {
    #[cfg(target_os = "macos")]
    type Prepared = crate::macos_update_install::RetainedBackup;
    #[cfg(not(target_os = "macos"))]
    type Prepared = ();
    fn stage(&self, target: &str, stage: &str) -> AppResult<()> {
        crate::update_receipt::write(&self.app, target, stage)?;
        crate::app_log::record(
            0,
            crate::app_log::Area::App,
            &format!(
                "软件更新 v{target}：{}",
                crate::update_receipt::stage_label(stage)
            ),
        );
        Ok(())
    }
    fn preflight(&self, candidate: &Candidate) -> AppResult<Self::Prepared> {
        crate::tun_service::preflight_app_upgrade(&self.app)?;
        installation_preflight(&self.app, candidate.size)?;
        #[cfg(target_os = "macos")]
        {
            let backup = crate::macos_update_install::Backup::prepare()?;
            crate::update_receipt::set_backup(&self.app, backup.path())?;
            backup.retain(&candidate.update.version)
        }
        #[cfg(not(target_os = "macos"))]
        Ok(())
    }
    fn stop(&self) -> AppResult<()> {
        let app = &self.app;
        app.state::<crate::local_routing::LocalRoutingManager>()
            .shutdown(app)?;
        app.state::<crate::openai_stability::StabilityManager>()
            .stop();
        crate::platform::restore_system_proxy(app)?;
        app.state::<MihomoRuntime>().stop(Some(app))?;
        app.state::<crate::OpenAiPolicyTaskManager>().cancel()?;
        app.state::<crate::GlobalTrafficMonitor>().stop();
        AppStorage::from_app(app)?.mark_clean_shutdown(true)?;
        Ok(())
    }
    fn prepare_services(&self, target: &str) -> AppResult<()> {
        crate::tun_service::prepare_app_upgrade(&self.app, target)
    }
    fn replace(
        &self,
        candidate: &Candidate,
        bytes: Vec<u8>,
        _prepared: Self::Prepared,
    ) -> AppResult<()> {
        let replace = || {
            candidate
                .update
                .install(bytes)
                .map_err(|e| failure(format!("安装启动失败：{e}")))
        };
        #[cfg(target_os = "macos")]
        {
            _prepared.install(replace)
        }
        #[cfg(not(target_os = "macos"))]
        {
            replace()
        }
    }
    fn rollback_services(&self) -> AppResult<()> {
        crate::tun_service::rollback_app_upgrade(&self.app)
    }
    fn restart(&self) -> AppResult<()> {
        #[cfg(not(windows))]
        self.app.restart();
        #[cfg(windows)]
        Ok(())
    }
    fn failed(&self, target: &str, stage: &str, reason: &str, disrupted: bool) -> AppResult<()> {
        crate::update_receipt::failed(&self.app, target, stage, reason, disrupted)
    }
}
