//! One AppKit-owned status item and a non-activating quick panel. No webview,
//! event tap, activation-policy changes or core I/O on the menu-bar click.

use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAccessibility, NSBackingStoreType, NSButton, NSColor, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventType, NSFont, NSFontWeightMedium, NSFontWeightSemibold, NSImage,
    NSLineBreakMode, NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSStatusBar, NSStatusBarButton,
    NSStatusItem, NSTextAlignment, NSTextField, NSVariableStatusItemLength,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
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

pub(super) const SIZE: NSSize = NSSize::new(304.0, 370.0);
const CORNER: f64 = 14.0;
const INSET: f64 = 8.0;
const ACTION_EVENTS: NSEventMask = NSEventMask::LeftMouseDown.union(NSEventMask::RightMouseDown);
const OUTSIDE_EVENTS: NSEventMask = ACTION_EVENTS.union(NSEventMask::OtherMouseDown);
const NOT_SAMPLED: &str = "正在采集流量…";
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

const SYSTEM: isize = 0;
const TUN: isize = 1;
const ROUTE: isize = 2;
const REFRESH: isize = 3;
const START: isize = 4;
const STOP: isize = 5;
const OPEN: isize = 6;
const QUIT: isize = 7;

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
            msg_send![super(this), initWithContentRect: rect(0.0, 0.0, SIZE.width, SIZE.height),
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered, defer: false]
        };
        panel.setTitle(&NSString::from_str("Serylane 快捷面板"));
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
        self.makeKeyAndOrderFront(None);
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

pub(super) fn panel_origin(size: NSSize, anchor: NSRect, visible: NSRect) -> NSPoint {
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
pub(super) fn outside_frames(point: NSPoint, panel: NSRect, anchor: Option<NSRect>) -> bool {
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
    dismiss_if_outside(mtm, screen);
}

pub(super) fn dismiss_if_outside(_: MainThreadMarker, screen: NSPoint) {
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

type ActionHandler = Arc<dyn Fn(Action) + Send + Sync>;

struct PanelState {
    panel: Retained<QuickPanel>,
    status_button: Retained<NSStatusBarButton>,
    traffic: Retained<NSTextField>,
    status: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    rows: RefCell<Vec<Retained<NSButton>>>,
    controls: RefCell<Controls>,
    confirming: Cell<Option<RouteToken>>,
    queued: Cell<bool>,
    on_action: ActionHandler,
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
            if !state.panel.is_open() || state.panel.isKeyWindow() { return; }
            // Clicking the status item itself can resign the panel *before*
            // the button action. Let that same click toggle it closed once.
            if button_anchor(&state.status_button)
                .is_some_and(|(anchor, _)| contains(anchor, NSEvent::mouseLocation()))
            {
                return;
            }
            state.panel.dismiss();
        }
    }

    impl Target {
        #[unsafe(method(toggleSerylanePanel:))]
        fn toggle(&self, sender: &NSStatusBarButton) {
            let state = self.ivars();
            if state.panel.is_open() {
                state.panel.dismiss();
            } else if let Some((anchor, screen)) = button_anchor(sender) {
                state.confirming.set(None);
                render(self);
                state.panel.open_at(anchor, &screen);
                self.dispatch(Action::Observe);
            }
        }

        #[unsafe(method(serylanePanelAction:))]
        fn select_action(&self, sender: &NSButton) {
            let state = self.ivars();
            if !state.panel.is_open() || !sender.isEnabled() { return; }
            let action = {
                let view = state.controls.borrow();
                let confirming = state.confirming.get();
                match sender.tag() {
                    OPEN => Action::OpenWindow,
                    QUIT => Action::Quit,
                    _ if state.queued.get() || view.busy.is_some() => return,
                    SYSTEM => Action::SystemProxy(!view.system_proxy),
                    TUN => Action::Tun(!view.tun),
                    ROUTE => {
                        let Some(route) = &view.route else { return };
                        if !route.can_toggle {
                            Action::OpenRoute
                        } else if confirming == Some(route.token) {
                            state.confirming.set(None);
                            Action::Route(route.token)
                        } else {
                            state.confirming.set(Some(route.token));
                            drop(view);
                            render(self);
                            return;
                        }
                    }
                    REFRESH if confirming.is_some() => {
                        state.confirming.set(None);
                        drop(view);
                        render(self);
                        return;
                    }
                    REFRESH => Action::Refresh,
                    START => Action::Start,
                    STOP => Action::Stop,
                    _ => return,
                }
            };
            if matches!(action, Action::OpenWindow | Action::OpenRoute | Action::Quit) {
                state.panel.dismiss();
            } else {
                state.confirming.set(None);
                state.queued.set(true);
                render(self);
            }
            self.dispatch(action);
        }
    }
);

impl Target {
    fn dispatch(&self, action: Action) {
        let state = self.ivars();
        let handler = Arc::clone(&state.on_action);
        let alive = Arc::clone(&state.alive);
        let block = RcBlock::new(move || {
            if alive.load(Ordering::Acquire) {
                handler(action);
            }
        });
        // SAFETY: the block's captures are Send + Sync; the queue is main.
        unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
    }
}

fn render(target: &Target) {
    let state = target.ivars();
    let view = state.controls.borrow();
    let rows = state.rows.borrow();
    let busy = view.busy.is_some() || state.queued.get();
    let progress = match view.busy {
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
    state.status.setStringValue(&NSString::from_str(progress));
    let route = view.route.as_ref();
    let confirmation = state
        .confirming
        .get()
        .and_then(|token| route.filter(|r| r.token == token));
    let route_title = if let Some(r) = confirmation {
        if r.token.enabled {
            "确认开启路由"
        } else {
            "确认关闭路由"
        }
    } else {
        route.map_or("路由 · 待核对", |r| {
            if r.can_toggle {
                r.label
            } else {
                "路由 · 待配置 ›"
            }
        })
    };
    let route_detail = confirmation.map(|r| {
        if r.token.enabled {
            format!(
                "将应用已保存的 {} 条域名 → {}，仅影响新连接。",
                r.domains, r.target
            )
        } else {
            "将移除域名覆盖，其他规则保持不变。".into()
        }
    });
    let detail = route_detail.as_deref().unwrap_or_else(|| {
        if !view.detail.is_empty() {
            &view.detail
        } else {
            route.map_or("", |r| {
                if r.message == "已关闭" {
                    "路由未开启"
                } else {
                    r.message.as_str()
                }
            })
        }
    });
    state
        .detail
        .setStringValue(&NSString::from_str(if busy { "" } else { detail }));
    state.detail.setToolTip(Some(&NSString::from_str(detail)));
    for (index, title, checked, enabled, tip) in [
        (
            SYSTEM,
            "系统代理",
            view.system_proxy,
            view.can_switch && confirmation.is_none(),
            "",
        ),
        (
            TUN,
            "TUN 模式",
            view.tun,
            (view.tun || view.can_switch && view.tun_supported) && confirmation.is_none(),
            view.tun_hint.as_str(),
        ),
        (
            ROUTE,
            route_title,
            route.is_some_and(|r| r.active) && confirmation.is_none(),
            route.is_some(),
            route.map_or("", |r| r.message.as_str()),
        ),
        (
            REFRESH,
            if confirmation.is_some() {
                "取消确认"
            } else {
                "刷新状态"
            },
            false,
            true,
            "仅刷新当前代理与路由状态",
        ),
        (
            START,
            "启动代理",
            false,
            view.can_start && confirmation.is_none(),
            "使用已保存的模式和活动配置",
        ),
        (
            STOP,
            "停止代理",
            false,
            view.can_stop && confirmation.is_none(),
            "停止代理并保留路由设置",
        ),
        (OPEN, "打开主窗口", false, true, "保留原页面"),
        (QUIT, "退出 Serylane", false, confirmation.is_none(), ""),
    ] {
        let row = &rows[index as usize];
        let label = format!("{}  {}", if checked { "✓" } else { "  " }, title);
        row.setTitle(&NSString::from_str(&label));
        row.setAccessibilityLabel(Some(&NSString::from_str(title)));
        row.setEnabled(enabled && (!busy || index == OPEN));
        row.setToolTip(
            (!tip.is_empty())
                .then(|| NSString::from_str(tip))
                .as_deref(),
        );
    }
}

fn label(text: &str, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setFrame(frame);
    field
}

fn populate_panel(target: &Target, mtm: MainThreadMarker) {
    let state = target.ivars();
    let panel = &state.panel;
    let surface = NSVisualEffectView::initWithFrame(
        NSVisualEffectView::alloc(mtm),
        rect(0.0, 0.0, SIZE.width, SIZE.height),
    );
    surface.setMaterial(NSVisualEffectMaterial::Popover);
    surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    surface.setState(NSVisualEffectState::Active);
    surface.setWantsLayer(true);
    round_layer(&surface, CORNER);
    if let Some(layer) = surface.layer() {
        // SAFETY: rendered in the same layer as the blur mask. CGColor is
        // borrowed from a retained NSColor for the duration of the setter.
        let separator = NSColor::separatorColor();
        unsafe {
            let color: *const std::ffi::c_void = msg_send![&*separator, CGColor];
            let _: () = msg_send![&*layer, setBorderColor: color];
            let _: () = msg_send![&*layer, setBorderWidth: 0.7_f64];
        }
    }
    let heading = label("Serylane", rect(17.0, 330.0, 270.0, 23.0), mtm);
    unsafe {
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(
            17.0,
            NSFontWeightSemibold,
        )));
        state
            .traffic
            .setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                11.0,
                NSFontWeightMedium,
            )));
    }
    state.status.setFrame(rect(17.0, 306.0, 270.0, 21.0));
    state.traffic.setFrame(rect(17.0, 285.0, 270.0, 17.0));
    state
        .traffic
        .setTextColor(Some(&NSColor::secondaryLabelColor()));
    surface.addSubview(&heading);
    surface.addSubview(&state.status);
    surface.addSubview(&state.traffic);

    for y in [277.0, 188.0, 53.0] {
        let line = label("", rect(13.0, y, SIZE.width - 26.0, 1.0), mtm);
        line.setDrawsBackground(true);
        line.setBackgroundColor(Some(&NSColor::separatorColor()));
        surface.addSubview(&line);
    }

    state.detail.setFrame(rect(17.0, 57.0, 270.0, 38.0));
    state.detail.setMaximumNumberOfLines(3);
    state
        .detail
        .setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    state
        .detail
        .setTextColor(Some(&NSColor::secondaryLabelColor()));
    state.detail.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    surface.addSubview(&state.detail);

    panel.setContentView(Some(&surface));
    let mut rows: Vec<Retained<NSButton>> = vec![];
    for (index, y) in [249.0, 221.0, 193.0, 160.0, 133.0, 106.0, 29.0, 4.0]
        .into_iter()
        .enumerate()
    {
        // SAFETY: Target implements serylanePanelAction: for this NSButton.
        let row = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(""),
                Some(target),
                Some(sel!(serylanePanelAction:)),
                mtm,
            )
        };
        row.setTag(index as isize);
        row.setFrame(rect(12.0, y, SIZE.width - 24.0, 25.0));
        row.setBordered(false);
        row.setAlignment(NSTextAlignment::Left);
        row.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        surface.addSubview(&row);
        if let Some(previous) = rows.last() {
            // SAFETY: all rows share the panel's lifetime.
            unsafe { previous.setNextKeyView(Some(&row)) };
        } else {
            panel.setInitialFirstResponder(Some(&row));
        }
        rows.push(row);
    }
    if let (Some(first), Some(last)) = (rows.first(), rows.last()) {
        unsafe { last.setNextKeyView(Some(first)) }; // same content view
    }
    *state.rows.borrow_mut() = rows;
    panel.setDelegate(Some(ProtocolObject::from_ref(target)));
    render(target);
}

fn round_layer(surface: &NSVisualEffectView, radius: f64) {
    if let Some(layer) = surface.layer() {
        // SAFETY: CGFloat is a double on both supported macOS architectures.
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: radius];
        }
        layer.setMasksToBounds(true);
    }
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
        traffic: NSTextField::labelWithString(
            &NSString::from_str(if traffic_enabled {
                NOT_SAMPLED
            } else {
                "流量显示已关闭"
            }),
            mtm,
        ),
        status: NSTextField::labelWithString(&NSString::from_str(""), mtm),
        detail: NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm),
        rows: RefCell::new(Vec::new()),
        controls: RefCell::new(Controls::default()),
        confirming: Cell::new(None),
        queued: Cell::new(false),
        on_action: Arc::new(on_action),
        alive: Arc::new(AtomicBool::new(true)),
    };
    let target: Retained<Target> =
        unsafe { msg_send![super(Target::alloc(mtm).set_ivars(state)), init] };
    populate_panel(&target, mtm);

    button.setImage(Some(&brand));
    button.setToolTip(Some(&NSString::from_str("Serylane")));
    button.setAccessibilityLabel(Some(&NSString::from_str("Serylane 快捷面板")));
    // SAFETY: NativeStatus keeps Target alive, and it implements the selector
    // for precisely this NSStatusBarButton. Do not attach an NSMenu too.
    unsafe {
        button.setTarget(Some(&target));
        button.setAction(Some(sel!(toggleSerylanePanel:)));
    }
    button.sendActionOn(ACTION_EVENTS);

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
        });
    });
    Ok(())
}

pub(super) fn is_visible() -> bool {
    PANEL_VISIBLE.load(Ordering::Acquire)
}

/// Newer state supersedes old requests, including late reads after a write.
pub(super) fn update_controls(_: MainThreadMarker, view: Controls) {
    let target = STATUS.with_borrow(|owned| owned.as_ref().map(|s| s.target.clone()));
    if let Some(target) = target {
        let state = target.ivars();
        {
            let current = state.controls.borrow();
            if view.serial <= current.serial {
                return;
            }
            // A slow quiet read cannot unlock a row whose click was just
            // queued. Only a shared action transition releases the latch.
            if view.action_serial > current.action_serial {
                state.queued.set(false);
            }
        }
        if state
            .confirming
            .get()
            .is_some_and(|token| view.route.as_ref().is_none_or(|r| r.token != token))
        {
            state.confirming.set(None);
        }
        *state.controls.borrow_mut() = view;
        render(&target);
    }
}

/// Rejected before the service admitted the action, so no action event is due.
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
        render(&target);
    }
}

pub(super) fn with_item<R>(
    _: MainThreadMarker,
    f: impl FnOnce(&NSStatusItem, &NSImage, &NSTextField) -> R,
) -> Option<R> {
    // Keep callbacks out of the TLS borrow; AppKit can deliver notifications
    // synchronously when it lays out the resized status item.
    let (item, brand, traffic) = STATUS.with_borrow(|owned| {
        owned.as_ref().map(|owned| {
            (
                owned.item.clone(),
                owned.brand.clone(),
                owned.target.ivars().traffic.clone(),
            )
        })
    })?;
    Some(f(&item, &brand, &traffic))
}

pub(super) fn uninstall(_: MainThreadMarker) {
    if let Some(owned) = STATUS.with_borrow_mut(Option::take) {
        let state = owned.target.ivars();
        state.alive.store(false, Ordering::Release);
        // SAFETY: exactly the registered observer and action target owned here.
        unsafe {
            let observer: &ProtocolObject<dyn NSObjectProtocol> = &owned.space_observer;
            owned.space_center.removeObserver(observer.as_ref());
            state.status_button.setTarget(None);
            state.status_button.setAction(None);
        }
        state.panel.setDelegate(None);
        state.panel.dismiss();
        owned.bar.removeStatusItem(&owned.item);
    }
}
