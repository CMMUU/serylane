//! AppKit owns the status item and non-activating panel. The shared Rust
//! controller owns operations and verified status.

use crate::i18n;
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAccessibility, NSBackingStoreType, NSButton, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSFont,
    NSImage, NSLineBreakMode, NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSStatusBar,
    NSStatusBarButton, NSStatusItem, NSTextAlignment, NSTextField, NSUserInterfaceLayoutDirection,
    NSVariableStatusItemLength, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowCollectionBehavior, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSOperationQueue, NSPoint, NSRect, NSSize, NSString,
};
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use uuid::Uuid;

const WIDTH: f64 = 336.0;
const PADDING: f64 = 14.0;
const CORNER: f64 = 14.0;
const INSET: f64 = 8.0;
const OUTSIDE_EVENTS: NSEventMask = NSEventMask::LeftMouseDown
    .union(NSEventMask::RightMouseDown)
    .union(NSEventMask::OtherMouseDown);
static PANEL_VISIBLE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RouteToken {
    pub enabled: bool,
    pub revision: u64,
    pub profile: Option<Uuid>,
    pub profile_revision: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub(super) struct RouteRow {
    pub token: RouteToken,
    pub label: &'static str,
    pub message: String,
    pub can_toggle: bool,
    pub active: bool,
    pub domains: usize,
    pub target: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Controls {
    pub serial: u64,
    pub action_serial: u64,
    pub busy: Option<&'static str>,
    pub status: &'static str,
    pub can_switch: bool,
    pub can_start: bool,
    pub can_stop: bool,
    pub system_proxy: bool,
    pub tun: bool,
    pub tun_supported: bool,
    pub tun_hint: String,
    pub route: Option<RouteRow>,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Observe,
    SystemProxy(bool),
    Tun(bool),
    Route(RouteToken),
    Refresh,
    Start,
    Stop,
    OpenWindow,
    OpenRoute,
    Quit,
}

pub(super) const STATUS_ROW: isize = 1;
pub(super) const TRAFFIC: isize = 2;
pub(super) const SYSTEM: isize = 3;
pub(super) const TUN: isize = 4;
pub(super) const ROUTE: isize = 5;
pub(super) const REFRESH: isize = 6;
pub(super) const START: isize = 7;
pub(super) const STOP: isize = 8;
pub(super) const DETAIL: isize = 9;
pub(super) const OPEN: isize = 10;
pub(super) const QUIT: isize = 11;
pub(super) const ROUTE_INFO: isize = 12;
pub(super) const ROUTE_CONFIRM: isize = 13;
pub(super) const ROUTE_SETTINGS: isize = 14;

#[derive(Clone, Copy, Default)]
struct Presented {
    system_proxy: bool,
    tun: bool,
    route: Option<RouteToken>,
}

#[derive(Default)]
struct Visibility {
    open: Cell<bool>,
    monitors: RefCell<Vec<Retained<AnyObject>>>,
}

define_class!(
    #[unsafe(super = NSPanel)]
    #[name = "SerylaneQuickPanel"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Visibility]
    struct QuickPanel;

    impl QuickPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }

        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::KeyDown
                && event.keyCode() == 53
                && !event.modifierFlags().intersects(
                    NSEventModifierFlags::Command
                        | NSEventModifierFlags::Control
                        | NSEventModifierFlags::Option,
                )
            {
                self.dismiss();
                return;
            }
            // SAFETY: pass the same borrowed event to NSPanel's implementation.
            unsafe { msg_send![super(self), sendEvent: event] }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _: Option<&AnyObject>) {
            self.dismiss();
        }
    }
);

impl QuickPanel {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Visibility::default());
        // SAFETY: initialize the allocated subclass with NSPanel's designated
        // initializer, and never use the consumed allocation again.
        let panel: Retained<Self> = unsafe {
            msg_send![super(this), initWithContentRect: rect(0.0, 0.0, WIDTH, 1.0),
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false]
        };
        panel.setAutorecalculatesKeyViewLoop(true);
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setHidesOnDeactivate(false);
        // SAFETY: the retained owner is the only code that releases the panel.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSPopUpMenuWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::CanJoinAllApplications
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel
    }

    fn open_at(&self, anchor: NSRect, display: &NSScreen) {
        self.dismiss(); // clear any tokens left after a system-ordered hide
        self.setFrameOrigin(panel_origin(
            self.frame().size,
            anchor,
            display.visibleFrame(),
        ));
        self.ivars().open.set(true);
        self.invalidateShadow();
        // Order into the current Space without activating Serylane. The
        // non-activating panel can still receive keyboard input while open.
        self.orderFrontRegardless();
        self.makeKeyWindow();
        if self.is_open() {
            PANEL_VISIBLE.store(true, Ordering::Release);
            self.watch_outside();
        }
    }

    fn is_open(&self) -> bool {
        self.ivars().open.get() && self.isVisible()
    }

    fn dismiss(&self) {
        // orderOut can deliver windowDidResignKey synchronously.
        if self.ivars().open.replace(false) {
            PANEL_VISIBLE.store(false, Ordering::Release);
            let monitors = self.ivars().monitors.take();
            for monitor in monitors {
                // SAFETY: token obtained from AppKit immediately below.
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
            self.orderOut(None);
        }
    }

    fn watch_outside(&self) {
        let local = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: AppKit supplies a live NSEvent for this callback. Returning
            // the original pointer leaves the user's click delivery untouched.
            outside_event(unsafe { event.as_ref() });
            event.as_ptr()
        });
        let global = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: AppKit owns the event until this callback returns.
            outside_event(unsafe { event.as_ref() });
        });
        let monitors = [
            unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(OUTSIDE_EVENTS, &local)
            },
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(OUTSIDE_EVENTS, &global),
        ];
        self.ivars()
            .monitors
            .borrow_mut()
            .extend(monitors.into_iter().flatten());
    }
}

fn panel_origin(size: NSSize, anchor: NSRect, visible: NSRect) -> NSPoint {
    let min_x = visible.origin.x + INSET;
    let max_x = visible.origin.x + visible.size.width - INSET - size.width;
    let min_y = visible.origin.y + INSET;
    let top = visible.origin.y + visible.size.height - INSET;
    NSPoint::new(
        (anchor.origin.x + (anchor.size.width - size.width) / 2.0)
            .min(max_x)
            .max(min_x),
        ((anchor.origin.y - INSET).min(top) - size.height).max(min_y),
    )
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

fn button_anchor(button: &NSStatusBarButton) -> Option<(NSRect, Retained<NSScreen>)> {
    let window = button.window()?;
    let anchor = window.convertRectToScreen(button.convertRect_toView(button.bounds(), None));
    if anchor.size.width <= 0.0 || anchor.size.height <= 0.0 {
        return None;
    }
    let centre = NSPoint::new(
        anchor.origin.x + anchor.size.width / 2.0,
        anchor.origin.y + anchor.size.height / 2.0,
    );
    let screen = NSScreen::screens(button.mtm())
        .iter()
        .find(|screen| contains(screen.frame(), centre))
        .or_else(|| window.screen())?;
    Some((anchor, screen))
}

/// Shared hit-testing for local and cross-application mouse delivery.
fn outside_frames(point: NSPoint, panel: NSRect, anchor: Option<NSRect>) -> bool {
    if anchor.is_some_and(|frame| contains(frame, point)) {
        return false; // The status button's own click performs the toggle.
    }
    if !contains(panel, point) {
        return true;
    }
    let radius = CORNER
        .min(panel.size.width / 2.0)
        .min(panel.size.height / 2.0);
    let x = point.x - panel.origin.x;
    let y = point.y - panel.origin.y;
    let dx = (radius - x).max(x - (panel.size.width - radius)).max(0.0);
    let dy = (radius - y).max(y - (panel.size.height - radius)).max(0.0);
    dx * dx + dy * dy > radius * radius
}

fn outside_event(event: &NSEvent) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let point = event.locationInWindow();
    let screen = event
        .window(mtm)
        .map_or(point, |window| window.convertPointToScreen(point));
    dismiss_if_outside(screen);
}

fn dismiss_if_outside(screen: NSPoint) {
    let pair = STATUS.with_borrow(|owned| {
        owned.as_ref().map(|value| {
            let state = value.target.ivars();
            (state.panel.clone(), state.status_button.clone())
        })
    });
    let Some((panel, status)) = pair else { return };
    if panel.is_open()
        && outside_frames(
            screen,
            panel.frame(),
            button_anchor(&status).map(|(anchor, _)| anchor),
        )
    {
        panel.dismiss();
    }
}

struct PanelState {
    panel: Retained<QuickPanel>,
    status_button: Retained<NSStatusBarButton>,
    surface: Retained<NSVisualEffectView>,
    traffic: Retained<NSTextField>,
    traffic_label: Retained<NSTextField>,
    controls: RefCell<Controls>,
    presented: Cell<Presented>,
    route_expanded: Cell<bool>,
    queued: Cell<bool>,
    on_action: Arc<dyn Fn(Action) + Send + Sync>,
    alive: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "SerylaneQuickPanelTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelState]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSWindowDelegate for Target {
        #[unsafe(method(windowDidResignKey:))]
        fn resign_key(&self, _: &NSNotification) {
            let state = self.ivars();
            // The status button owns its click even if it resigns the panel
            // before sending the toggle action.
            if state.panel.is_open() && !state.panel.isKeyWindow()
                && !button_anchor(&state.status_button)
                    .is_some_and(|(anchor, _)| contains(anchor, NSEvent::mouseLocation()))
            {
                state.panel.dismiss();
            }
        }
    }

    impl Target {
        #[unsafe(method(toggleSerylanePanel:))]
        fn toggle(&self, sender: &NSStatusBarButton) {
            let state = self.ivars();
            if !state.alive.load(Ordering::Acquire) {
                return;
            }
            if state.panel.is_open() {
                state.panel.dismiss();
            } else if let Some((anchor, screen)) = button_anchor(sender) {
                state.route_expanded.set(false);
                render(self, true);
                state.panel.open_at(anchor, &screen);
                if state.panel.is_open() {
                    (state.on_action)(Action::Observe);
                }
            }
        }

        #[unsafe(method(serylanePanelAction:))]
        fn select_action(&self, sender: &NSButton) {
            let state = self.ivars();
            if !state.panel.is_open() || sender.isHidden() || !sender.isEnabled()
                || !state.alive.load(Ordering::Acquire)
            {
                return;
            }
            if sender.tag() == ROUTE {
                state.route_expanded.set(!state.route_expanded.get());
                render(self, false);
                return;
            }
            let view = state.controls.borrow();
            let shown = state.presented.get();
            let action = match sender.tag() {
                OPEN => Action::OpenWindow,
                QUIT => Action::Quit,
                ROUTE_SETTINGS => Action::OpenRoute,
                _ if state.queued.get() || view.busy.is_some() => return,
                SYSTEM if view.can_switch && shown.system_proxy == view.system_proxy =>
                    Action::SystemProxy(!shown.system_proxy),
                TUN if view.can_switch && view.tun_supported && shown.tun == view.tun =>
                    Action::Tun(!shown.tun),
                ROUTE_CONFIRM => {
                    let Some(route) = view.route.as_ref().filter(|route|
                        route.can_toggle && Some(route.token) == shown.route
                    ) else { return };
                    Action::Route(route.token)
                }
                REFRESH => Action::Refresh,
                START if view.can_start => Action::Start,
                STOP if view.can_stop => Action::Stop,
                _ => return,
            };
            drop(view);
            state.panel.dismiss();
            if !matches!(action, Action::OpenWindow | Action::OpenRoute | Action::Quit) {
                state.queued.set(true);
                render(self, false);
            }
            let handler = Arc::clone(&state.on_action);
            let alive = Arc::clone(&state.alive);
            let block = RcBlock::new(move || {
                if alive.load(Ordering::Acquire) {
                    handler(action);
                }
            });
            // SAFETY: the captures are Send + Sync. Dispatch after AppKit
            // has returned from the control's action callback.
            unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
        }
    }
);

const SEPARATOR_STATUS: isize = 15;
const SEPARATOR_SWITCHES: isize = 16;
const SEPARATOR_COMMANDS: isize = 17;

// Top to bottom, in points. Hidden rows occupy no space.
const ROWS: [(isize, f64); 17] = [
    (STATUS_ROW, 25.0),
    (TRAFFIC, 20.0),
    (SEPARATOR_STATUS, 11.0),
    (SYSTEM, 27.0),
    (TUN, 27.0),
    (ROUTE, 27.0),
    (ROUTE_INFO, 54.0),
    (ROUTE_CONFIRM, 27.0),
    (ROUTE_SETTINGS, 27.0),
    (SEPARATOR_SWITCHES, 11.0),
    (REFRESH, 27.0),
    (START, 27.0),
    (STOP, 27.0),
    (DETAIL, 44.0),
    (SEPARATOR_COMMANDS, 11.0),
    (OPEN, 27.0),
    (QUIT, 27.0),
];

fn populate(target: &Target) {
    let state = target.ivars();
    let mtm = target.mtm();
    for (tag, _) in ROWS {
        match tag {
            TRAFFIC => {
                state.traffic_label.setTag(TRAFFIC);
                state
                    .traffic_label
                    .setFont(Some(&NSFont::systemFontOfSize(12.0)));
                state
                    .traffic_label
                    .setTextColor(Some(&NSColor::secondaryLabelColor()));
                state.surface.addSubview(&state.traffic_label);
            }
            STATUS_ROW | ROUTE_INFO | DETAIL | SEPARATOR_STATUS | SEPARATOR_SWITCHES
            | SEPARATOR_COMMANDS => {
                let empty = NSString::from_str("");
                let label = if matches!(tag, ROUTE_INFO | DETAIL) {
                    let label = NSTextField::wrappingLabelWithString(&empty, mtm);
                    label.setMaximumNumberOfLines(3);
                    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                    label
                } else {
                    let label = NSTextField::labelWithString(&empty, mtm);
                    if tag == STATUS_ROW {
                        label.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
                        label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    } else {
                        label.setDrawsBackground(true);
                        label.setBackgroundColor(Some(&NSColor::separatorColor()));
                    }
                    label
                };
                label.setTag(tag);
                label.setSelectable(false);
                state.surface.addSubview(&label);
            }
            _ => {
                let is_checkbox = matches!(tag, SYSTEM | TUN);
                let create = if is_checkbox {
                    NSButton::checkboxWithTitle_target_action
                } else {
                    NSButton::buttonWithTitle_target_action
                };
                // SAFETY: the retained Target implements this selector for NSButton.
                let button = unsafe {
                    create(
                        &NSString::from_str(""),
                        Some(target),
                        Some(sel!(serylanePanelAction:)),
                        mtm,
                    )
                };
                button.setTag(tag);
                if !is_checkbox {
                    button.setBordered(false);
                }
                button.setFont(Some(&NSFont::systemFontOfSize(14.0)));
                button.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                state.surface.addSubview(&button);
            }
        }
    }
    state.panel.setContentView(Some(&state.surface));
    state
        .panel
        .setDelegate(Some(ProtocolObject::from_ref(target)));
    render(target, true);
}

fn new_surface(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let surface = NSVisualEffectView::initWithFrame(
        NSVisualEffectView::alloc(mtm),
        rect(0.0, 0.0, WIDTH, 1.0),
    );
    surface.setMaterial(NSVisualEffectMaterial::Popover);
    surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    surface.setState(NSVisualEffectState::Active);
    surface.setWantsLayer(true);
    if let Some(layer) = surface.layer() {
        // SAFETY: CGFloat is a double on supported macOS architectures.
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: CORNER];
        }
        layer.setMasksToBounds(true);
    }
    surface
}

fn row(state: &PanelState, tag: isize) -> Retained<NSButton> {
    state
        .surface
        .viewWithTag(tag)
        .and_then(|row| row.downcast().ok())
        .expect("panel button")
}

fn field(state: &PanelState, tag: isize) -> Retained<NSTextField> {
    state
        .surface
        .viewWithTag(tag)
        .and_then(|row| row.downcast().ok())
        .expect("panel label")
}

fn title(row: &NSButton, text: &str) {
    let text = NSString::from_str(&i18n::text(text));
    if *row.title() != *text {
        row.setTitle(&text);
    }
}

fn checked(row: &NSButton, yes: bool) {
    row.setState(if yes {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
}

fn layout(state: &PanelState) {
    let visible: Vec<_> = ROWS
        .iter()
        .filter_map(|&(tag, height)| {
            let view = state.surface.viewWithTag(tag).expect("panel row");
            (!view.isHidden()).then_some((view, tag, height))
        })
        .collect();
    let height = 2.0 * PADDING + visible.iter().map(|(_, _, height)| *height).sum::<f64>();
    let size = NSSize::new(WIDTH, height);
    let mut y = height - PADDING;
    for (view, tag, row_height) in visible {
        y -= row_height;
        let (origin, height) = if matches!(
            tag,
            SEPARATOR_STATUS | SEPARATOR_SWITCHES | SEPARATOR_COMMANDS
        ) {
            (y + (row_height - 1.0) / 2.0, 1.0)
        } else {
            (y, row_height)
        };
        view.setFrame(rect(PADDING, origin, WIDTH - 2.0 * PADDING, height));
    }
    if state.panel.frame().size != size {
        if let Some((anchor, display)) =
            button_anchor(&state.status_button).filter(|_| state.panel.is_open())
        {
            let origin = panel_origin(size, anchor, display.visibleFrame());
            state
                .panel
                .setFrame_display(NSRect::new(origin, size), true);
            state.panel.invalidateShadow();
        } else {
            state.panel.setContentSize(size);
        }
        state.panel.recalculateKeyViewLoop();
    }
}

fn render(target: &Target, update_presented: bool) {
    let state = target.ivars();
    // No borrow across the native setters, which can dispatch AppKit callbacks.
    let view = state.controls.borrow().clone();
    let route = view.route.as_ref();
    if update_presented {
        state.presented.set(Presented {
            system_proxy: view.system_proxy,
            tun: view.tun,
            route: route.filter(|r| r.can_toggle).map(|r| r.token),
        });
    }
    let shown = state.presented.get();
    let busy = view.busy.is_some() || state.queued.get();
    let status = match view.busy {
        Some("starting") => "正在开启代理…",
        Some("switching") => "正在切换网络…",
        Some("stopping") => "正在关闭代理…",
        Some("routing") => "正在应用路由…",
        Some("refreshing") => "正在刷新状态…",
        Some(_) => "正在处理…",
        None if state.queued.get() => "正在提交…",
        None if view.status.is_empty() => "正在读取代理状态…",
        None => view.status,
    };
    let (direction, alignment) = if i18n::right_to_left() {
        (
            NSUserInterfaceLayoutDirection::RightToLeft,
            NSTextAlignment::Right,
        )
    } else {
        (
            NSUserInterfaceLayoutDirection::LeftToRight,
            NSTextAlignment::Left,
        )
    };
    if state.surface.userInterfaceLayoutDirection() != direction {
        state.surface.setUserInterfaceLayoutDirection(direction);
    }
    for (tag, _) in ROWS {
        let view = state.surface.viewWithTag(tag).expect("panel row");
        view.setUserInterfaceLayoutDirection(direction);
        if let Ok(button) = view.clone().downcast::<NSButton>() {
            button.setAlignment(alignment);
        } else if let Ok(label) = view.downcast::<NSTextField>() {
            label.setAlignment(alignment);
        }
    }
    field(state, STATUS_ROW).setStringValue(&NSString::from_str(&format!(
        "Serylane · {}",
        i18n::text(status)
    )));
    for (tag, label) in [
        (SYSTEM, "系统代理"),
        (TUN, "TUN 模式"),
        (ROUTE_SETTINGS, "打开路由设置…"),
        (REFRESH, "刷新状态"),
        (START, "启动代理"),
        (STOP, "停止代理"),
        (OPEN, "打开主窗口"),
        (QUIT, "退出 Serylane"),
    ] {
        title(&row(state, tag), label);
    }
    refresh_traffic(target);

    let system = row(state, SYSTEM);
    checked(&system, view.system_proxy);
    system.setEnabled(!busy && view.can_switch && shown.system_proxy == view.system_proxy);

    let tun = row(state, TUN);
    checked(&tun, view.tun);
    tun.setEnabled(!busy && view.can_switch && view.tun_supported && shown.tun == view.tun);
    tun.setToolTip(
        (!view.tun_hint.is_empty())
            .then(|| NSString::from_str(&i18n::text(&view.tun_hint)))
            .as_deref(),
    );

    let route_item = row(state, ROUTE);
    let route_label = route.map_or("路由 · 待核对", |r| {
        if r.can_toggle {
            r.label
        } else {
            "路由 · 待配置"
        }
    });
    let expanded = state.route_expanded.get();
    let arrow = if expanded {
        "▾"
    } else if i18n::right_to_left() {
        "◂"
    } else {
        "▸"
    };
    let check = if route.is_some_and(|r| r.active) {
        "✓ "
    } else {
        ""
    };
    route_item.setTitle(&NSString::from_str(&format!(
        "{check}{}   {arrow}",
        i18n::text(route_label)
    )));
    let confirm = row(state, ROUTE_CONFIRM);
    let matching = route.filter(|r| r.can_toggle && Some(r.token) == shown.route);
    let info = match (route, matching) {
        (Some(r), Some(_)) if r.token.enabled => {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("count", r.domains);
            args.set("target", r.target.as_str());
            i18n::message("native-apply-routes", Some(&args)).unwrap_or_else(|| {
                format!(
                    "将应用已保存的 {} 条域名 → {}，仅影响新连接。",
                    r.domains, r.target
                )
            })
        }
        (Some(_), Some(_)) => "将移除域名覆盖，其他规则保持不变。".into(),
        (Some(r), None) if r.can_toggle => "配置有变化，请重新打开菜单确认。".into(),
        (Some(r), None) => r.message.clone(),
        (None, _) => "正在核对路由状态…".into(),
    };
    let info_label = field(state, ROUTE_INFO);
    let info = NSString::from_str(&i18n::text(&info));
    info_label.setStringValue(&info);
    info_label.setToolTip(Some(&info));
    info_label.setHidden(!expanded);
    confirm.setHidden(!expanded);
    row(state, ROUTE_SETTINGS).setHidden(!expanded);
    title(
        &confirm,
        if shown.route.is_some_and(|r| !r.enabled) {
            "确认关闭路由"
        } else {
            "确认开启路由"
        },
    );
    confirm.setEnabled(!busy && matching.is_some());
    row(state, REFRESH).setEnabled(!busy);
    row(state, START).setEnabled(!busy && view.can_start);
    row(state, STOP).setEnabled(!busy && view.can_stop);
    let detail = field(state, DETAIL);
    let localized = i18n::text(&view.detail);
    let message = localized.trim();
    let text = NSString::from_str(message);
    detail.setStringValue(&text);
    detail.setHidden(message.is_empty());
    detail.setToolTip((!message.is_empty()).then_some(&*text));
    layout(state);
}

struct NativeStatus {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
    brand: Retained<NSImage>,
    target: Retained<Target>,
    space_center: Retained<NSNotificationCenter>,
    space_observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

thread_local! {
    static STATUS: RefCell<Option<NativeStatus>> = const { RefCell::new(None) };
}

pub(super) fn install(
    mtm: MainThreadMarker,
    brand: Retained<NSImage>,
    traffic_enabled: bool,
    on_action: impl Fn(Action) + Send + Sync + 'static,
) -> Result<(), &'static str> {
    if STATUS.with_borrow(Option::is_some) {
        return Err("已登记状态项");
    }
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSVariableStatusItemLength);
    let Some(button) = item.button(mtm) else {
        bar.removeStatusItem(&item);
        return Err("菜单栏按钮不可用");
    };
    let state = PanelState {
        panel: QuickPanel::new(mtm),
        status_button: button.clone(),
        surface: new_surface(mtm),
        traffic: NSTextField::labelWithString(
            &NSString::from_str(if traffic_enabled {
                "正在采集流量…"
            } else {
                "流量显示已关闭"
            }),
            mtm,
        ),
        traffic_label: NSTextField::labelWithString(&NSString::from_str(""), mtm),
        controls: RefCell::new(Controls::default()),
        presented: Cell::new(Presented::default()),
        route_expanded: Cell::new(false),
        queued: Cell::new(false),
        on_action: Arc::new(on_action),
        alive: Arc::new(AtomicBool::new(true)),
    };
    let target: Retained<Target> =
        unsafe { msg_send![super(Target::alloc(mtm).set_ivars(state)), init] };
    populate(&target);
    refresh_accessibility(&target);
    button.setImage(Some(&brand));
    button.setToolTip(Some(&NSString::from_str("Serylane")));
    // SAFETY: NativeStatus retains Target for as long as the button has this selector.
    unsafe {
        button.setTarget(Some(&target));
        button.setAction(Some(sel!(toggleSerylanePanel:)));
    }
    button.sendActionOn(NSEventMask::LeftMouseUp.union(NSEventMask::RightMouseUp));
    let space_center = NSWorkspace::sharedWorkspace().notificationCenter();
    let block = RcBlock::new(|_: NonNull<NSNotification>| {
        if MainThreadMarker::new().is_some() {
            let panel = STATUS.with_borrow(|owned| {
                owned
                    .as_ref()
                    .map(|owned| owned.target.ivars().panel.clone())
            });
            if let Some(panel) = panel {
                if panel.is_open() && !panel.isOnActiveSpace() {
                    panel.dismiss();
                }
            }
        }
    });
    // SAFETY: this block captures nothing and UI access is restricted to the
    // main queue; the observer token is removed when the status item is removed.
    let space_observer = unsafe {
        space_center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceActiveSpaceDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    };
    STATUS.with_borrow_mut(|status| {
        *status = Some(NativeStatus {
            bar,
            item,
            brand,
            target,
            space_center,
            space_observer,
        })
    });
    Ok(())
}

fn refresh_accessibility(target: &Target) {
    let state = target.ivars();
    let label = NSString::from_str(&i18n::text("Serylane 快捷菜单"));
    state.status_button.setAccessibilityLabel(Some(&label));
    state.panel.setTitle(&label);
}

fn refresh_traffic(target: &Target) {
    let state = target.ivars();
    let translated = NSString::from_str(&i18n::text(&state.traffic.stringValue().to_string()));
    let field = field(state, TRAFFIC);
    if *field.stringValue() != *translated {
        field.setStringValue(&translated);
    }
}

pub(super) fn refresh_locale(_: MainThreadMarker) {
    let target = STATUS.with_borrow(|owned| owned.as_ref().map(|s| s.target.clone()));
    if let Some(target) = target {
        refresh_accessibility(&target);
        render(&target, !is_visible());
    }
}

pub(super) fn is_visible() -> bool {
    PANEL_VISIBLE.load(Ordering::Acquire)
}

pub(super) fn update_controls(_: MainThreadMarker, view: Controls) {
    let target = STATUS.with_borrow(|owned| owned.as_ref().map(|s| s.target.clone()));
    if let Some(target) = target {
        let state = target.ivars();
        {
            let mut current = state.controls.borrow_mut();
            if view.serial <= current.serial {
                return;
            }
            // Quiet reads must not unlock an action queued on the main run loop.
            if view.action_serial > current.action_serial {
                state.queued.set(false);
            }
            *current = view;
        }
        render(&target, !is_visible());
    }
}

pub(super) fn reject_queued(_: MainThreadMarker, action_serial: u64, detail: String) {
    let target = STATUS.with_borrow(|owned| owned.as_ref().map(|s| s.target.clone()));
    if let Some(target) = target {
        let state = target.ivars();
        let mut view = state.controls.borrow_mut();
        if view.action_serial != action_serial || !state.queued.replace(false) {
            return;
        }
        if view.detail.is_empty() && view.busy.is_none() {
            view.detail = detail;
        }
        drop(view);
        render(&target, !is_visible());
    }
}

pub(super) fn with_item<R>(
    _: MainThreadMarker,
    f: impl FnOnce(&NSStatusItem, &NSImage, &NSTextField) -> R,
) -> Option<R> {
    let (item, brand, target) = STATUS.with_borrow(|owned| {
        owned
            .as_ref()
            .map(|s| (s.item.clone(), s.brand.clone(), s.target.clone()))
    })?;
    let result = f(&item, &brand, &target.ivars().traffic);
    refresh_traffic(&target);
    Some(result)
}

pub(super) fn uninstall(_: MainThreadMarker) {
    if let Some(owned) = STATUS.with_borrow_mut(Option::take) {
        let state = owned.target.ivars();
        state.alive.store(false, Ordering::Release);
        // SAFETY: remove the exact observer retained during installation.
        unsafe {
            owned
                .space_center
                .removeObserver(owned.space_observer.as_ref())
        };
        state.panel.setDelegate(None);
        state.panel.dismiss();
        if let Some(button) = owned.item.button(owned.target.mtm()) {
            // SAFETY: Clear the selector and target before releasing the owner.
            unsafe {
                button.setAction(None);
                button.setTarget(None);
            }
        }
        owned.bar.removeStatusItem(&owned.item);
    }
    PANEL_VISIBLE.store(false, Ordering::Release);
}

#[cfg(test)]
#[allow(dead_code, reason = "used by the standalone AppKit test executable")]
pub(super) fn test_panel() -> Option<Retained<objc2_app_kit::NSWindow>> {
    STATUS.with_borrow(|owned| {
        owned
            .as_ref()
            .map(|s| Retained::into_super(Retained::into_super(s.target.ivars().panel.clone())))
    })
}
