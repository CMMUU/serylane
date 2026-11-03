//! Opt-in native acceptance fixture. No production setup, IPC commands,
//! configuration storage, plugins or proxy processes are loaded.
//! Run twice, with/without --companion, then use the companion's green window
//! button to test another application's fullscreen Space. Close both afterward.
#[cfg(target_os = "macos")]
#[path = "../src/macos_interaction.rs"]
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
        println!("journal {level}: {message}");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::TrayIconBuilder,
        Listener, Manager,
    };
    let companion = std::env::args().any(|arg| arg == "--companion");
    let title = if companion {
        "Fullscreen Companion"
    } else {
        "Serylane Interaction Fixture"
    };
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = if companion {
        "com.cmmuu.fixture.companion"
    } else {
        "com.cmmuu.fixture.interaction"
    }
    .into();
    context.config_mut().product_name = Some(title.into());
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default().setup(move |app| {
        app.listen("window-reveal-status", |event| println!("reveal: {}", event.payload()));
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External("about:blank".parse()?))
            .title(title).inner_size(700.0, 440.0)
            .initialization_script(format!("addEventListener('DOMContentLoaded',()=>{{document.title={title:?};document.body.innerHTML='<main style=\"font:24px system-ui;padding:50px\"><h1>{title}</h1><p>Isolated native interaction test</p><p>No proxy, Tailscale or user configuration access.</p><input placeholder=\"Focus and typing test\"></main>';}});"))
            .build()?;
        if !companion {
            let show = MenuItem::with_id(app, "show", "显示测试窗口", true, None::<&str>)?;
            let hide = MenuItem::with_id(app, "hide", "隐藏测试窗口", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出测试", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &hide, &quit])?;
            let tray = TrayIconBuilder::with_id("fixture").title("QA").tooltip(title)
                .icon(macos_layout::brand_icon().clone()).icon_as_template(true)
                .menu(&menu).show_menu_on_left_click(true)
                .on_menu_event(|app, event| { println!("menu: {}", event.id.as_ref()); match event.id.as_ref() {
                    "show" => macos_interaction::reveal(app),
                    "hide" => { let _ = app.get_webview_window("main").unwrap().hide(); },
                    "quit" => app.exit(0), _ => {}
                }}).build(app)?;
            macos_interaction::install(&tray).map_err(std::io::Error::other)?;
            println!("native menu installed; pid={}", std::process::id());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                for n in 0..900 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let handle2 = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if let Some(tray) = handle2.tray_by_id("fixture") {
                            if !macos_title::menu_tracking(&tray) {
                                let _ = macos_title::update(&tray, 6000 + n * 30, 8000 + n * 50);
                            }
                        }
                    });
                }
            });
        }
        Ok(())
    }).build(context).expect("isolated native fixture");
    app.run(move |app, event| {
        if matches!(event, tauri::RunEvent::Reopen { .. }) && !companion {
            macos_interaction::reveal(app);
        }
    });
}
#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macOS-only opt-in native acceptance fixture");
}
