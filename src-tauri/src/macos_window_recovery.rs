use crate::{app_log, i18n};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSWindow, NSWorkspace,
    NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSBundle, NSString};
use std::{sync::Mutex, time::Duration};
use tauri::{webview::PageLoadEvent, AppHandle, Manager, Url, Webview, WebviewWindow};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Page {
    #[default]
    Loading,
    Ready,
    Interrupted,
    Recovering(u64),
    Failed,
    Prompting,
}

impl Page {
    fn terminated(self) -> Self {
        match self {
            Self::Loading | Self::Ready => Self::Interrupted,
            Self::Recovering(_) => Self::Failed,
            _ => self,
        }
    }
}

#[derive(Default)]
pub(super) struct MainWindowRecovery(Mutex<(Page, u64)>);

pub(super) fn bring_to_front(window: &WebviewWindow) {
    let (Some(marker), Ok(native)) = (MainThreadMarker::new(), window.ns_window()) else {
        return;
    };
    // SAFETY: Tauri's main-thread window handle owns this live NSWindow.
    let native = unsafe { &*native.cast::<NSWindow>() };
    if native.isOnActiveSpace() && NSApplication::sharedApplication(marker).isActive() {
        return;
    }
    let bundle = NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none() {
        return;
    }
    // An explicit open should follow the same Space transition as the Dock.
    // Reuse this running bundle and leave the loaded page intact.
    let configuration = NSWorkspaceOpenConfiguration::new();
    configuration.setActivates(true);
    configuration.setCreatesNewApplicationInstance(false);
    configuration.setAddsToRecentItems(false);
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &bundle.bundleURL(),
        &configuration,
        None,
    );
}

fn entry_url(app: &AppHandle) -> Url {
    if tauri::is_dev() {
        if let Some(url) = &app.config().build.dev_url {
            return url.clone();
        }
    }
    Url::parse("tauri://localhost/").expect("bundled macOS app URL")
}

pub(super) fn loaded(webview: &Webview, payload: &tauri::webview::PageLoadPayload<'_>) {
    if webview.label() != "main" || !matches!(payload.event(), PageLoadEvent::Finished) {
        return;
    }
    let entry = entry_url(webview.app_handle());
    let url = payload.url();
    if url.scheme() == entry.scheme() && url.host() == entry.host() && url.port() == entry.port() {
        let recovery = webview.state::<MainWindowRecovery>();
        let mut state = recovery.0.lock().unwrap();
        if matches!(
            state.0,
            Page::Recovering(_) | Page::Failed | Page::Prompting
        ) {
            app_log::record(1, app_log::Area::App, "macOS 主窗口页面加载已恢复");
        }
        state.0 = Page::Ready;
    }
}

pub(super) fn terminated(webview: &Webview) {
    if webview.label() != "main" {
        return;
    }
    let recovery = webview.state::<MainWindowRecovery>();
    {
        let mut state = recovery.0.lock().unwrap();
        state.0 = state.0.terminated();
    }
    app_log::record(2, app_log::Area::App, "macOS 主窗口 WebKit 内容进程已终止");
    let app = webview.app_handle().clone();
    // Dispatch after the navigation delegate returns, including at application startup.
    tauri::async_runtime::spawn(async move {
        let dispatcher = app.clone();
        let _ = dispatcher.run_on_main_thread(move || resume_visible(&app));
    });
}

fn resume_visible(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            resume(&window);
        }
    }
}

pub(super) fn resume(window: &WebviewWindow) {
    let recovery = window.state::<MainWindowRecovery>();
    let next = {
        let mut state = recovery.0.lock().unwrap();
        match state.0 {
            Page::Interrupted => {
                state.1 += 1;
                state.0 = Page::Recovering(state.1);
            }
            Page::Failed => state.0 = Page::Prompting,
            _ => return,
        }
        state.0
    };
    if let Page::Recovering(generation) = next {
        // reload() has no target if WebKit crashed before committing its first document.
        if window.navigate(entry_url(window.app_handle())).is_err() {
            failed(window.app_handle(), generation);
            return;
        }
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(12)).await;
            let dispatcher = app.clone();
            let _ = dispatcher.run_on_main_thread(move || failed(&app, generation));
        });
    } else {
        let retry = MainThreadMarker::new().is_some_and(|mtm| {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(&i18n::text("主窗口未能加载")));
            alert.addButtonWithTitle(&NSString::from_str(&i18n::text("重试")));
            alert.addButtonWithTitle(&NSString::from_str(&i18n::text("关闭")));
            alert.runModal() == NSAlertFirstButtonReturn
        });
        let mut state = recovery.0.lock().unwrap();
        if state.0 == Page::Prompting {
            state.0 = if retry {
                Page::Interrupted
            } else {
                Page::Failed
            };
            drop(state);
            if retry {
                resume(window);
            }
        }
    }
}

fn failed(app: &AppHandle, generation: u64) {
    let recovery = app.state::<MainWindowRecovery>();
    let mut state = recovery.0.lock().unwrap();
    if state.0 == Page::Recovering(generation) {
        state.0 = Page::Failed;
        drop(state);
        resume_visible(app);
    }
}

#[cfg(test)]
mod tests {
    use super::Page;

    #[test]
    fn cold_and_running_terminations_recover_but_a_second_failure_waits_for_retry() {
        assert_eq!(Page::Loading.terminated(), Page::Interrupted);
        assert_eq!(Page::Ready.terminated(), Page::Interrupted);
        assert_eq!(Page::Recovering(1).terminated(), Page::Failed);
        assert_eq!(Page::Failed.terminated(), Page::Failed);
        assert_eq!(Page::Prompting.terminated(), Page::Prompting);
    }
}
