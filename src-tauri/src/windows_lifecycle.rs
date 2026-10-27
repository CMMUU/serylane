//! Installed on the owning UI thread. A normal close/minimize is not shutdown.
use crate::lifecycle::{Lifecycle, PROCESS_LIFECYCLE};
use tauri::{AppHandle, Emitter, Manager};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION,
};

const SUBCLASS_ID: usize = 0x53455259;
struct Context {
    app: Option<AppHandle>,
}

pub fn install(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主窗口未创建")?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    // setup runs on the window-owning thread, before background work starts.
    unsafe { install_on(hwnd.0.cast(), Some(app.clone())) }.map_err(|e| e.to_string())
}

unsafe fn install_on(hwnd: HWND, app: Option<AppHandle>) -> std::io::Result<()> {
    let context = Box::into_raw(Box::new(Context { app }));
    // SAFETY: caller owns this thread's live HWND. WM_NCDESTROY releases context.
    if unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, context as usize) } == 0 {
        unsafe {
            drop(Box::from_raw(context));
        }
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn transition(gate: &Lifecycle, message: u32, wparam: WPARAM) -> bool {
    match message {
        WM_QUERYENDSESSION => gate.prepare(),
        WM_ENDSESSION if wparam == 0 => gate.cancel(),
        WM_ENDSESSION => {
            gate.close();
            true
        }
        _ => false,
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: installed context lives until the WM_NCDESTROY branch below.
    let context = unsafe { &*(data as *const Context) };
    if transition(&PROCESS_LIFECYCLE, message, wparam) {
        if let Some(app) = &context.app {
            if message == WM_ENDSESSION && wparam != 0 {
                // Do cleanup before returning control to Windows/Tao. Its total
                // waiting budget is bounded; the later Exit callback is idempotent.
                crate::cleanup_app(app);
            } else {
                // Do not write a journal or wait for runtime locks in QUERY.
                let snapshot = PROCESS_LIFECYCLE.snapshot();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = app.emit("app-lifecycle", snapshot);
                    crate::app_log::record(
                        0,
                        crate::app_log::Area::App,
                        &format!(
                            "lifecycle phase={} generation={}",
                            snapshot.phase, snapshot.generation
                        ),
                    );
                });
            }
        }
    }
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(hwnd, Some(window_proc), id);
        }
        unsafe {
            drop(Box::from_raw(data as *mut Context));
        }
    }
    // Forward even shutdown: keep Tao's native exit behavior and other hooks.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SendMessageW, WM_CLOSE,
    };

    #[test]
    fn close_to_tray_is_not_a_shutdown_request() {
        let gate = Lifecycle::new();
        assert!(!transition(&gate, WM_CLOSE, 0));
        assert!(gate.admitting());
    }

    #[test]
    fn native_session_messages_preserve_cancel_and_confirm() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_lifecycle::tests::native_session_fixture",
                "--nocapture",
            ])
            .env("SERYLANE_SESSION_FIXTURE", "1")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("session-fixture-ok"));
    }

    #[test]
    fn native_session_fixture() {
        if std::env::var("SERYLANE_SESSION_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // Only this hidden test window receives messages. No shutdown/logoff API
        // or broadcast is used; the installed application is not involved.
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            install_on(hwnd, None).unwrap();
            let old = PROCESS_LIFECYCLE.ticket().unwrap();
            assert_ne!(SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0), 0);
            assert!(!PROCESS_LIFECYCLE.admitting());
            SendMessageW(hwnd, WM_ENDSESSION, 0, 0);
            assert!(PROCESS_LIFECYCLE.admitting());
            assert!(PROCESS_LIFECYCLE.check(old).is_err());
            SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0);
            SendMessageW(hwnd, WM_ENDSESSION, 1, 0);
            SendMessageW(hwnd, WM_ENDSESSION, 0, 0);
            assert_eq!(PROCESS_LIFECYCLE.snapshot().phase, "closing");
            assert!(PROCESS_LIFECYCLE.admission().is_err());
            assert_ne!(DestroyWindow(hwnd), 0);
        }
        println!("session-fixture-ok");
    }
}
