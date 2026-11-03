//! Prefer AppKit's native status-menu path. Fullscreen Space behavior needs
//! separate interactive acceptance; WebView/network reads never gate this menu.
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSStatusItem, NSWindow};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager};

static REVEAL: AtomicU64 = AtomicU64::new(0);

pub fn use_native_menu(item: &NSStatusItem, mtm: MainThreadMarker) -> Result<usize, String> {
    if item.menu(mtm).is_none() {
        return Err("native status menu is missing".into());
    }
    let button = item.button(mtm).ok_or("native status button is missing")?;
    let views = button.subviews();
    let count = views
        .iter()
        .filter(|view| view.class().name().to_bytes() == b"TaoTrayTarget")
        .count();
    if count != 1 {
        return Err("tray event-view contract changed; leave the original view untouched".into());
    }
    for view in views {
        // tray-icon 0.24 installs this event-only overlay. We deliberately use
        // AppKit's real button/menu event path on macOS, not synthetic clicks.
        // Keep the owned view alive for dependency setters; only stop hit tests.
        if view.class().name().to_bytes() == b"TaoTrayTarget" {
            view.setHidden(true);
        }
    }
    Ok(count)
}

pub fn install(tray: &tauri::tray::TrayIcon) -> Result<(), String> {
    tray.with_inner_tray_icon(|inner| {
        let mtm = MainThreadMarker::new().ok_or("tray setup is not on the main thread")?;
        let item = inner
            .ns_status_item()
            .ok_or("native status item is missing")?;
        match use_native_menu(&item, mtm)? {
            1 => Ok(()),
            _ => Err(
                "tray dependency event-view contract changed; native menu needs verification"
                    .into(),
            ),
        }
    })
    .map_err(|error| error.to_string())?
}

pub fn reveal(app: &AppHandle) {
    let started = std::time::Instant::now();
    let sequence = REVEAL.fetch_add(1, Ordering::AcqRel) + 1;
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        if sequence != REVEAL.load(Ordering::Acquire) {
            return;
        }
        let result = (|| -> Result<(), String> {
            let mtm = MainThreadMarker::new().ok_or("window restore is not on the main thread")?;
            let application = NSApplication::sharedApplication(mtm);
            application.unhideWithoutActivation();
            let window = handle
                .get_webview_window("main")
                .ok_or("main window is missing")?;
            window.unminimize().map_err(|e| e.to_string())?;
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            // Activation and Space transitions are asynchronous. Do not retry
            // focus in a loop or move every window to all Spaces.
            Ok(())
        })();
        if result.is_err() {
            report(&handle, "failed", started);
            return;
        }
        let verify = handle.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            let observer = verify.clone();
            let _ = verify.run_on_main_thread(move || {
                if sequence != REVEAL.load(Ordering::Acquire) {
                    return;
                }
                let ready = observer.get_webview_window("main").is_some_and(|window| {
                    window.ns_window().ok().is_some_and(|raw| {
                        // The Tauri-owned window remains alive for this main-thread callback.
                        let native = unsafe { &*raw.cast::<NSWindow>() };
                        native.isVisible()
                            && !native.isMiniaturized()
                            && native.isKeyWindow()
                            && native.isOnActiveSpace()
                    })
                });
                report(&observer, if ready { "ready" } else { "pending" }, started);
            });
        });
    });
    if queued.is_err() {
        report(app, "failed", started);
    }
}

fn report(app: &AppHandle, status: &'static str, started: std::time::Instant) {
    let _ = app.emit("window-reveal-status", status);
    // The journal can touch disk: keep it off the AppKit event loop.
    let elapsed = started.elapsed().as_millis();
    tauri::async_runtime::spawn_blocking(move || {
        crate::app_log::record(
            u8::from(status != "ready"),
            crate::app_log::Area::App,
            &format!("主窗口恢复核对：{status}，{elapsed} ms（成功路径延后 750 ms 核对）"),
        )
    });
}
