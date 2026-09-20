//! Metadata-only polling. No profile activation, YAML revision or runtime access.
use crate::{
    models::{ProfileSource, SubscriptionStatus},
    storage::AppStorage,
    subscription::SubscriptionFetcher,
};
use chrono::Utc;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

pub const POLL_SECONDS: i64 = 300;

fn due(status: Option<&SubscriptionStatus>) -> bool {
    status.and_then(|s| s.checked_at).is_none_or(|at| {
        let age = (Utc::now() - at).num_seconds();
        !(0..POLL_SECONDS).contains(&age)
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuotaObservation {
    profile_id: Uuid,
    status: SubscriptionStatus,
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // One sequential worker avoids bursts against subscription services.
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            if app
                .state::<crate::session_resume::SessionResumeManager>()
                .is_shutting_down()
            {
                break;
            }
            let Ok(storage) = AppStorage::from_app(&app) else {
                continue;
            };
            let Ok(profiles) = storage.list_profiles() else {
                continue;
            };
            let Ok(fetcher) = SubscriptionFetcher::new() else {
                continue;
            };
            for profile in profiles {
                if app
                    .state::<crate::session_resume::SessionResumeManager>()
                    .is_shutting_down()
                {
                    return;
                }
                let ProfileSource::RemoteSubscription { url, user_agent } = &profile.source else {
                    continue;
                };
                if !due(storage.subscription_status(profile.id).as_ref()) {
                    continue;
                }
                let started = Utc::now();
                let result = fetcher.fetch_usage(url, user_agent).await;
                if app
                    .state::<crate::session_resume::SessionResumeManager>()
                    .is_shutting_down()
                {
                    return;
                }
                // A newer manual refresh wins even if this older request finishes last.
                let recorded = storage.record_subscription_check_since(
                    profile.id,
                    result.as_ref().ok().and_then(Option::as_ref),
                    result.as_ref().err(),
                    started,
                );
                if recorded.is_ok() {
                    if let Some(status) = storage.subscription_status(profile.id) {
                        let _ = app.emit(
                            "subscription-usage-updated",
                            QuotaObservation {
                                profile_id: profile.id,
                                status,
                            },
                        );
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_use_five_minute_cadence_even_on_failure_or_missing_data() {
        assert!(due(None));
        let mut status = SubscriptionStatus {
            checked_at: Some(Utc::now()),
            last_error: Some("HTTP 403".into()),
            ..Default::default()
        };
        assert!(!due(Some(&status)));
        status.checked_at = Some(Utc::now() - chrono::Duration::seconds(301));
        assert!(due(Some(&status)));
        status.checked_at = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(due(Some(&status)));
    }
}
