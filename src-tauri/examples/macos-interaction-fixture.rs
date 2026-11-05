//! Opt-in native acceptance fixture. No production setup, IPC commands,
//! configuration storage, plugins or proxy processes are loaded.
//! Run twice, with/without --companion, then use the companion's green window
//! button to test another application's fullscreen Space. Close both afterward.
#[cfg(target_os = "macos")]
#[path = "../src/macos_interaction.rs"]
#[allow(dead_code)]
mod macos_interaction;
#[cfg(target_os = "macos")]
#[path = "../src/traffic_monitor/macos_layout.rs"]
#[allow(dead_code)]
mod macos_layout;
#[cfg(target_os = "macos")]
#[path = "../src/traffic_monitor/macos_title.rs"]
#[allow(dead_code)]
mod macos_title;
#[cfg(target_os = "macos")]
mod app_log {
    pub enum Area {
        App,
    }
    pub fn record(level: u8, _: Area, message: &str) {
        super::qa_log(&format!("journal {level}: {message}"));
    }
}

#[cfg(target_os = "macos")]
fn qa_log(message: &str) {
    use std::{
        io::Write,
        os::unix::fs::OpenOptionsExt,
        sync::{Mutex, OnceLock},
    };
    static LOG: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    let log = LOG.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("serylane-native-qa-{}.log", std::process::id()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .ok()
            .map(Mutex::new)
    });
    println!("{message}");
    // Fixture-only evidence. No user text, real application data or networking.
    if let Some(log) = log {
        if let Ok(mut file) = log.lock() {
            let _ = writeln!(file, "{message}");
        }
    }
}

#[cfg(target_os = "macos")]
fn install_qa_controls(
    window: &tauri::WebviewWindow,
    title: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSFont, NSTextField, NSView, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let mtm = MainThreadMarker::new().ok_or("QA controls require main thread")?;
    let raw = window.ns_window()?;
    let native = unsafe { &*raw.cast::<NSWindow>() };
    let content = native.contentView().ok_or("QA content view missing")?;
    let bounds = content.bounds();
    let overlay = NSView::initWithFrame(NSView::alloc(mtm), bounds);
    overlay.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let heading = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    heading.setFrame(NSRect::new(
        NSPoint::new(40.0, bounds.size.height - 90.0),
        NSSize::new(620.0, 44.0),
    ));
    heading.setFont(Some(&NSFont::systemFontOfSize(24.0)));
    heading.setDrawsBackground(true);
    overlay.addSubview(&heading);
    let note = NSTextField::labelWithString(
        &NSString::from_str("Independent QA window. No proxy or user settings access."),
        mtm,
    );
    note.setFrame(NSRect::new(
        NSPoint::new(40.0, bounds.size.height - 140.0),
        NSSize::new(620.0, 30.0),
    ));
    note.setDrawsBackground(true);
    overlay.addSubview(&note);
    let field = NSTextField::initWithFrame(
        NSTextField::alloc(mtm),
        NSRect::new(
            NSPoint::new(40.0, bounds.size.height - 200.0),
            NSSize::new(400.0, 32.0),
        ),
    );
    field.setPlaceholderString(Some(&NSString::from_str("Focus and typing test")));
    field.setEditable(true);
    field.setEnabled(true);
    field.setBezeled(true);
    field.setDrawsBackground(true);
    overlay.addSubview(&field);
    content.addSubview(&overlay);
    let ready = field.window().is_some() && field.isEditable() && field.isEnabled();
    qa_log(&format!("native-qa-controls-ready={ready}"));
    Ok(ready)
}

#[cfg(target_os = "macos")]
fn main() {
    use tauri::{Listener, Manager};
    let companion = std::env::args().any(|arg| arg == "--companion")
        || std::env::var_os("SERYLANE_QA_COMPANION").is_some()
        || std::env::current_exe().ok().is_some_and(|p| {
            p.file_name()
                .is_some_and(|n| n == "fullscreen-companion-qa")
        });
    let traffic = std::env::args().any(|arg| arg == "--traffic");
    let force_failure = std::env::args().any(|arg| arg == "--self-test-force-failure");
    let self_test = force_failure || std::env::args().any(|arg| arg == "--self-test");
    let title = if companion {
        "Fullscreen Companion QA"
    } else {
        "Serylane Native Menu QA"
    };
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = if companion {
        "com.cmmuu.fixture.native-companion"
    } else {
        "com.cmmuu.fixture.native-menu"
    }
    .into();
    context.config_mut().product_name = Some(title.into());
    context.package_info_mut().name = title.into();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default().setup(move |app| {
        app.listen("window-reveal-status", |event| qa_log(&format!("reveal: {}", event.payload())));
        // The subject is native status-menu/Space/focus behavior, not WebKit
        // rendering. Keep a real Tauri window, with explicit AppKit QA controls;
        // do not load production assets or wait for a renderer/network process.
        let window = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External("about:blank".parse()?))
            .title(title).inner_size(700.0, 440.0).build()?;
        let controls_ok = install_qa_controls(&window, title)?;
        if !companion {
            use macos_interaction::MenuAction;
            let handler = app.handle().clone();
            macos_interaction::install(
                macos_layout::brand_icon(),
                &[("显示测试窗口", MenuAction::Show), ("隐藏测试窗口", MenuAction::Hide), ("退出测试", MenuAction::Quit)],
                Some("QA"),
                move |action| { qa_log(&format!("action: {action:?}")); match action {
                    MenuAction::Show => macos_interaction::reveal(&handler),
                    MenuAction::Hide => { macos_interaction::cancel_reveal(); let _ = handler.get_webview_window("main").unwrap().hide(); },
                    MenuAction::Quit => handler.exit(0), MenuAction::Stop => {},
                } },
                |trace| qa_log(&format!("native-trace {} {} ms", trace.stage, trace.elapsed_ms)),
            ).map_err(std::io::Error::other)?;
            qa_log(&format!("native menu installed; pid={}", std::process::id()));
            let handle = app.handle().clone();
            if traffic { tauri::async_runtime::spawn(async move {
                for n in 0..900 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let handle2 = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        let _ = handle2;
                        if !macos_interaction::menu_tracking() {
                            let _ = macos_title::update(6000 + n * 30, 8000 + n * 50);
                        }
                    });
                }
            }); }
        }
        qa_log(&format!("fixture-start companion={companion} pid={}", std::process::id()));
        if self_test {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let check = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    let window_ok = check.get_webview_window("main").is_some_and(|window| window.is_visible().unwrap_or(false));
                    let menu_ok = companion || macos_interaction::with_status_item(|item, mtm| Ok(item.menu(mtm).is_some_and(|menu| menu.numberOfItems() == 3))).unwrap_or(false);
                    qa_log(&format!("fixture-self-test window={window_ok} menu={menu_ok} controls={controls_ok}"));
                    if !(window_ok && menu_ok && controls_ok && !force_failure) {
                        macos_interaction::uninstall();
                        // The test process, not AppKit's termination result,
                        // owns the failure exit code. Never accept a blank UI.
                        std::process::exit(2);
                    }
                    check.exit(0);
                });
            });
        }

        let probe = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            let mut previous = None;
            for _ in 0..300 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let handle = probe.clone();
                let (send, receive) = tokio::sync::oneshot::channel();
                if probe.run_on_main_thread(move || {
                    let state = handle.get_webview_window("main").and_then(|window| window.ns_window().ok()).map(|raw| {
                        let window = unsafe { &*raw.cast::<objc2_app_kit::NSWindow>() };
                        (window.styleMask().contains(objc2_app_kit::NSWindowStyleMask::FullScreen), window.isOnActiveSpace(), window.isKeyWindow(), window.isVisible())
                    });
                    let _ = send.send(state);
                }).is_err() { break; }
                if let Ok(Ok(state)) = tokio::time::timeout(std::time::Duration::from_secs(2), receive).await {
                    if state != previous { qa_log(&format!("own-window-state companion={companion} fullscreen/active/key/visible={state:?}")); previous = state; }
                }
            }
        });
        Ok(())
    }).on_window_event(move |window, event| {
        if !companion {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                macos_interaction::cancel_reveal();
                let _ = window.hide();
            }
        }
    }).build(context).expect("isolated native fixture");
    app.run(move |app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            macos_interaction::uninstall();
        }
        if matches!(event, tauri::RunEvent::Reopen { .. }) && !companion {
            macos_interaction::reveal(app);
        }
    });
}
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macOS-only opt-in native acceptance fixture");
}
