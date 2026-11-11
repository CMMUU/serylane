//! One AppKit-owned status item. No tray-icon event overlays, synthetic clicks,
//! global event taps, activation-policy changes or network-dependent menus.
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSBitmapImageRep, NSCellImagePosition, NSDeviceRGBColorSpace, NSEvent,
    NSEventMask, NSEventType, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength, NSWindow,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSOperationQueue, NSSize, NSString};
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

static REVEAL: AtomicU64 = AtomicU64::new(0);
thread_local! {
    // AppKit objects never leave the main thread. Neither raw pointers nor
    // Retained<NSStatusItem> are placed in Tauri's Send + Sync application state.
    static STATUS: RefCell<Option<NativeStatus>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Show,
    Hide,
    Stop,
    Quit,
}
impl MenuAction {
    fn tag(self) -> isize {
        match self {
            Self::Show => 1,
            Self::Hide => 2,
            Self::Stop => 3,
            Self::Quit => 4,
        }
    }
    fn from_tag(tag: isize) -> Option<Self> {
        match tag {
            1 => Some(Self::Show),
            2 => Some(Self::Hide),
            3 => Some(Self::Stop),
            4 => Some(Self::Quit),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MenuTrace {
    pub stage: &'static str,
    pub elapsed_ms: u128,
}

type Handler = Arc<dyn Fn(MenuAction) + Send + Sync>;
type Journal = Arc<dyn Fn(MenuTrace) + Send + Sync>;

#[derive(Default)]
struct MenuSession {
    tracking: bool,
    pending: Option<MenuAction>,
}
impl MenuSession {
    fn select(&mut self, action: MenuAction) -> Option<MenuAction> {
        if self.tracking {
            self.pending = Some(action);
            None
        } else {
            Some(action)
        }
    }
    fn close(&mut self) -> Option<MenuAction> {
        self.tracking = false;
        self.pending.take()
    }
}

struct TargetState {
    session: RefCell<MenuSession>,
    click_at: RefCell<Option<Instant>>,
    on_action: Handler,
    journal: Journal,
    alive: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "SerylaneStatusMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}
    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuWillOpen:))]
        fn will_open(&self, _: &NSMenu) {
            self.ivars().session.borrow_mut().tracking = true;
            self.trace("menu-open");
        }
        #[unsafe(method(menuDidClose:))]
        fn did_close(&self, _: &NSMenu) {
            let pending = self.ivars().session.borrow_mut().close();
            self.trace("menu-close");
            if let Some(action) = pending { self.dispatch(action); }
        }
    }
    impl MenuTarget {
        #[unsafe(method(activateMenuItem:))]
        fn activate_item(&self, sender: &NSMenuItem) {
            if let Some(action) = MenuAction::from_tag(sender.tag()) {
                let ready = self.ivars().session.borrow_mut().select(action);
                self.trace("action-selected");
                if let Some(action) = ready { self.dispatch(action); }
            }
        }
    }
);

impl MenuTarget {
    fn trace(&self, stage: &'static str) {
        let elapsed_ms = self
            .ivars()
            .click_at
            .borrow()
            .map_or(0, |at| at.elapsed().as_millis());
        (self.ivars().journal)(MenuTrace { stage, elapsed_ms });
    }
    fn dispatch(&self, action: MenuAction) {
        self.trace("action-queued");
        let handler = self.ivars().on_action.clone();
        let alive = self.ivars().alive.clone();
        // Some AppKit versions deliver the action after menuDidClose, others
        // during tracking teardown. Always leave menu tracking before any focus,
        // shutdown or business operation. A queued action cannot revive an item
        // which has been uninstalled in the meantime.
        let block = RcBlock::new(move || {
            if alive.load(Ordering::Acquire) {
                handler(action);
            }
        });
        unsafe {
            NSOperationQueue::mainQueue().addOperationWithBlock(&block);
        }
    }
}

struct NativeStatus {
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
    monitor: Option<Retained<AnyObject>>,
}
impl Drop for NativeStatus {
    fn drop(&mut self) {
        self.target.ivars().alive.store(false, Ordering::Release);
        if let Some(monitor) = self.monitor.take() {
            // This is exactly the token returned by the local monitor API.
            unsafe {
                NSEvent::removeMonitor(&monitor);
            }
        }
        self.menu.cancelTracking();
        self.menu.setDelegate(None);
        for item in self.menu.itemArray() {
            // AppKit may temporarily retain a tracked menu after its status item
            // is removed. Clear its non-owning targets before releasing ours.
            unsafe {
                item.setTarget(None);
            }
        }
        self.item.setMenu(None);
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
        self.target.trace("uninstalled");
    }
}

pub fn install(
    icon: &tauri::image::Image<'_>,
    entries: &[(&str, MenuAction)],
    label: Option<&str>,
    on_action: impl Fn(MenuAction) + Send + Sync + 'static,
    journal: impl Fn(MenuTrace) + Send + Sync + 'static,
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("status item setup must run on the main thread")?;
    if STATUS.with_borrow(|state| state.is_some()) {
        return Err("status item is already installed".into());
    }
    let image = image_from_rgba(icon)?;
    image.setTemplate(true);
    let target = MenuTarget::alloc(mtm).set_ivars(TargetState {
        session: RefCell::new(MenuSession::default()),
        click_at: RefCell::new(None),
        on_action: Arc::new(on_action),
        journal: Arc::new(journal),
        alive: Arc::new(AtomicBool::new(true)),
    });
    let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for (title, action) in entries {
        // The selector has the correct single NSMenuItem argument. AppKit keeps
        // weak target/delegate references; NativeStatus owns their strong root.
        let entry = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(activateMenuItem:)),
                &NSString::from_str(""),
            )
        };
        entry.setTag(action.tag());
        unsafe {
            entry.setTarget(Some(&target));
        }
        entry.setEnabled(true);
        menu.addItem(&entry);
    }
    menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let mut owned = NativeStatus {
        item,
        menu,
        target,
        monitor: None,
    };
    let button = owned
        .item
        .button(mtm)
        .ok_or("native status button is missing")?;
    button.setImage(Some(&image));
    button.setImagePosition(NSCellImagePosition::ImageLeft);
    button.setToolTip(Some(&NSString::from_str("Serylane")));
    if let Some(label) = label {
        button.setTitle(&NSString::from_str(label));
    }
    // AppKit's own status button handles both mouse buttons and menu tracking.
    // No custom NSView is placed over it and no performClick is synthesized.
    owned.item.setMenu(Some(&owned.menu));
    let observed_button = button.clone();
    let observed_target = owned.target.clone();
    let observer = RcBlock::new(move |raw: std::ptr::NonNull<NSEvent>| {
        let event = unsafe { raw.as_ref() };
        if let Some(window) = observed_button.window() {
            if event.windowNumber() == window.windowNumber() {
                let point = observed_button.convertPoint_fromView(event.locationInWindow(), None);
                let bounds = observed_button.bounds();
                if point.x >= bounds.origin.x
                    && point.x < bounds.origin.x + bounds.size.width
                    && point.y >= bounds.origin.y
                    && point.y < bounds.origin.y + bounds.size.height
                {
                    *observed_target.ivars().click_at.borrow_mut() = Some(Instant::now());
                    observed_target.trace(if event.r#type() == NSEventType::RightMouseDown {
                        "right-down"
                    } else {
                        "left-down"
                    });
                }
            }
        }
        // Observation only: return the original event unchanged, always.
        raw.as_ptr()
    });
    owned.monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &observer,
        )
    };
    owned.target.trace(if owned.monitor.is_some() {
        "installed"
    } else {
        "installed-without-click-observer"
    });
    STATUS.with_borrow_mut(|state| *state = Some(owned));
    Ok(())
}

pub fn uninstall() {
    cancel_reveal();
    if MainThreadMarker::new().is_some() {
        let owned = STATUS.with_borrow_mut(Option::take);
        drop(owned);
    }
}

pub fn with_status_item<T>(
    apply: impl FnOnce(&NSStatusItem, MainThreadMarker) -> Result<T, String>,
) -> Result<T, String> {
    let mtm = MainThreadMarker::new().ok_or("status item access must run on the main thread")?;
    STATUS.with_borrow(|state| {
        let owned = state
            .as_ref()
            .ok_or("native status item is not installed")?;
        apply(&owned.item, mtm)
    })
}

pub fn menu_tracking() -> bool {
    MainThreadMarker::new().is_some()
        && STATUS.with_borrow(|state| {
            state
                .as_ref()
                .is_some_and(|owned| owned.target.ivars().session.borrow().tracking)
        })
}

pub fn image_from_rgba(rgba: &tauri::image::Image<'_>) -> Result<Retained<NSImage>, String> {
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let bytes = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(4))
        .ok_or("status icon dimensions overflow")?;
    if width == 0 || height == 0 || width > 2048 || height > 2048 || bytes != rgba.rgba().len() {
        return Err("invalid status icon RGBA dimensions".into());
    }
    let rep = unsafe { NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
        NSBitmapImageRep::alloc(), std::ptr::null_mut(), width as isize, height as isize,
        8, 4, true, false, NSDeviceRGBColorSpace, (width * 4) as isize, 32,
    ) }.ok_or("status icon bitmap allocation failed")?;
    // AppKit owns exactly the requested RGBA plane; dimensions and length were
    // checked above. Native images own the representation after this copy.
    unsafe {
        std::ptr::copy_nonoverlapping(rgba.rgba().as_ptr(), rep.bitmapData(), bytes);
    }
    let size = NSSize::new(width as f64 / 2.0, height as f64 / 2.0);
    rep.setSize(size);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);
    Ok(image)
}

pub fn set_icon(rgba: &tauri::image::Image<'_>, template: bool) -> Result<(), String> {
    let image = image_from_rgba(rgba)?;
    image.setTemplate(template);
    with_status_item(|item, mtm| {
        item.button(mtm)
            .ok_or("native status button is missing")?
            .setImage(Some(&image));
        Ok(())
    })
}

pub fn journal(trace: MenuTrace) {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app_log::record(
            0,
            crate::app_log::Area::App,
            &format!("macOS 菜单阶段：{}，{} ms", trace.stage, trace.elapsed_ms),
        );
    });
}

/// Explicit Show/Dock Reopen only. Opening the menu never activates the app.
/// Preserve the normal desktop/Dock and the user's window/Space preferences.
pub fn reveal(app: &AppHandle) {
    let started = Instant::now();
    let sequence = REVEAL.fetch_add(1, Ordering::AcqRel) + 1;
    let handle = app.clone();
    if app
        .run_on_main_thread(move || {
            if sequence != REVEAL.load(Ordering::Acquire) {
                return;
            }
            let result = (|| -> Result<(), String> {
                let mtm =
                    MainThreadMarker::new().ok_or("window restore is not on the main thread")?;
                NSApplication::sharedApplication(mtm).unhideWithoutActivation();
                let window = handle
                    .get_webview_window("main")
                    .ok_or("main window is missing")?;
                window.unminimize().map_err(|e| e.to_string())?;
                window.show().map_err(|e| e.to_string())?;
                window.set_focus().map_err(|e| e.to_string())?;
                Ok(())
            })();
            if result.is_err() {
                report(&handle, "failed", started);
                return;
            }
            let verify = handle.clone();
            tauri::async_runtime::spawn(async move {
                // Observe asynchronous Space activation, not a focus-stealing retry
                // loop. A close/hide/minimize cancels the observation generation.
                for _ in 0..12 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let observer = verify.clone();
                    let (send, receive) = tokio::sync::oneshot::channel();
                    if verify
                        .run_on_main_thread(move || {
                            if sequence != REVEAL.load(Ordering::Acquire) {
                                let _ = send.send(None);
                                return;
                            }
                            let ready = observer.get_webview_window("main").is_some_and(|window| {
                                window.ns_window().ok().is_some_and(|raw| {
                                    let native = unsafe { &*raw.cast::<NSWindow>() };
                                    native.isVisible()
                                        && !native.isMiniaturized()
                                        && native.isKeyWindow()
                                        && native.isOnActiveSpace()
                                })
                            });
                            let _ = send.send(Some(ready));
                        })
                        .is_err()
                    {
                        return;
                    }
                    match tokio::time::timeout(Duration::from_millis(500), receive).await {
                        Ok(Ok(Some(true))) => {
                            report(&verify, "ready", started);
                            return;
                        }
                        Ok(Ok(None)) => return,
                        Ok(Ok(Some(false))) => {}
                        _ => {
                            report(&verify, "event-loop-delayed", started);
                            return;
                        }
                    }
                }
                if sequence == REVEAL.load(Ordering::Acquire) {
                    report(&verify, "pending-space-or-focus", started);
                }
            });
        })
        .is_err()
    {
        report(app, "failed", started);
    }
}

pub fn cancel_reveal() {
    REVEAL.fetch_add(1, Ordering::AcqRel);
}

fn report(app: &AppHandle, status: &'static str, started: Instant) {
    let _ = app.emit("window-reveal-status", status);
    journal(MenuTrace {
        stage: status,
        elapsed_ms: started.elapsed().as_millis(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_actions_wait_for_tracking_to_close_and_dispatch_only_once() {
        let mut session = MenuSession {
            tracking: true,
            pending: None,
        };
        assert_eq!(session.select(MenuAction::Show), None);
        assert_eq!(session.close(), Some(MenuAction::Show));
        assert_eq!(session.close(), None);
        assert_eq!(session.select(MenuAction::Quit), Some(MenuAction::Quit));
    }
    #[test]
    fn dismissing_without_selection_never_reopens_the_window() {
        let mut session = MenuSession {
            tracking: true,
            pending: None,
        };
        assert_eq!(session.close(), None);
    }
    #[test]
    fn action_tags_are_closed_and_round_trip() {
        for action in [
            MenuAction::Show,
            MenuAction::Hide,
            MenuAction::Stop,
            MenuAction::Quit,
        ] {
            assert_eq!(MenuAction::from_tag(action.tag()), Some(action));
        }
        assert_eq!(MenuAction::from_tag(0), None);
        assert_eq!(MenuAction::from_tag(99), None);
    }
}
