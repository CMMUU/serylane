//! AppKit owns the status menu, its appearance, tracking and dismissal.
//! The same Rust controller owns every operation and verified status.

use crate::i18n;
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAccessibility, NSControlStateValueOff, NSControlStateValueOn, NSEventMask, NSImage, NSMenu,
    NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem, NSTextField,
    NSUserInterfaceLayoutDirection, NSVariableStatusItemLength,
};
use objc2_foundation::{NSOperationQueue, NSString};
use std::{
    cell::{Cell, RefCell},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use uuid::Uuid;

static MENU_VISIBLE: AtomicBool = AtomicBool::new(false);

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

struct MenuState {
    menu: Retained<NSMenu>,
    route_menu: Retained<NSMenu>,
    traffic: Retained<NSTextField>,
    controls: RefCell<Controls>,
    presented: Cell<Presented>,
    queued: Cell<bool>,
    on_action: Arc<dyn Fn(Action) + Send + Sync>,
    alive: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "SerylaneNativeStatusMenu"]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuState]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            if std::ptr::eq(menu, &*self.ivars().menu) {
                render(self, !is_visible());
            }
        }

        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _: &NSMenu) {
            if !MENU_VISIBLE.swap(true, Ordering::AcqRel) {
                // Observe only schedules the existing asynchronous controller.
                // No window activation or system status I/O in menu tracking.
                (self.ivars().on_action)(Action::Observe);
            }
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _: &NSMenu) {
            MENU_VISIBLE.store(false, Ordering::Release);
        }
    }

    impl Target {
        #[unsafe(method(serylaneMenuAction:))]
        fn select_action(&self, sender: &NSMenuItem) {
            let state = self.ivars();
            if !sender.isEnabled() || !state.alive.load(Ordering::Acquire) {
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
            // SAFETY: all captures are Send + Sync. Defer selection until the
            // native tracking callback returns (especially Show and Quit).
            unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
        }
    }
);

fn add_row(target: &Target, menu: &NSMenu, tag: isize, title: &str, action: bool) {
    // SAFETY: the allocated item belongs to the main thread and has no
    // untyped selector until its explicitly retained target is installed.
    let row = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(target.mtm()),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    };
    row.setTag(tag);
    if action {
        // SAFETY: NativeStatus owns the target until the menu is detached.
        unsafe {
            row.setTarget(Some(target));
            row.setAction(Some(sel!(serylaneMenuAction:)));
        }
    }
    row.setEnabled(action);
    menu.addItem(&row);
}

fn populate(target: &Target) {
    let state = target.ivars();
    let menu = &state.menu;
    let submenu = &state.route_menu;
    menu.setAutoenablesItems(false);
    submenu.setAutoenablesItems(false);
    add_row(target, menu, STATUS_ROW, "", false);
    add_row(target, menu, TRAFFIC, "", false);
    menu.addItem(&NSMenuItem::separatorItem(target.mtm()));
    add_row(target, menu, SYSTEM, "系统代理", true);
    add_row(target, menu, TUN, "TUN 模式", true);
    add_row(target, menu, ROUTE, "路由", false);
    add_row(target, submenu, ROUTE_INFO, "", false);
    add_row(target, submenu, ROUTE_CONFIRM, "确认开启路由", true);
    submenu.addItem(&NSMenuItem::separatorItem(target.mtm()));
    add_row(target, submenu, ROUTE_SETTINGS, "打开路由设置…", true);
    row(menu, ROUTE).setSubmenu(Some(submenu));
    menu.addItem(&NSMenuItem::separatorItem(target.mtm()));
    add_row(target, menu, REFRESH, "刷新状态", true);
    add_row(target, menu, START, "启动代理", true);
    add_row(target, menu, STOP, "停止代理", true);
    add_row(target, menu, DETAIL, "", false);
    menu.addItem(&NSMenuItem::separatorItem(target.mtm()));
    add_row(target, menu, OPEN, "打开主窗口", true);
    add_row(target, menu, QUIT, "退出 Serylane", true);
    render(target, true);
}

fn row(menu: &NSMenu, tag: isize) -> Retained<NSMenuItem> {
    menu.itemWithTag(tag).expect("native menu row")
}

fn title(row: &NSMenuItem, text: &str) {
    let text = NSString::from_str(&i18n::text(text));
    if *row.title() != *text {
        row.setTitle(&text);
    }
}

fn checked(row: &NSMenuItem, yes: bool) {
    row.setState(if yes {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
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
    let menu = &state.menu;
    let direction = if i18n::right_to_left() {
        NSUserInterfaceLayoutDirection::RightToLeft
    } else {
        NSUserInterfaceLayoutDirection::LeftToRight
    };
    menu.setUserInterfaceLayoutDirection(direction);
    state.route_menu.setUserInterfaceLayoutDirection(direction);
    title(
        &row(menu, STATUS_ROW),
        &format!("Serylane · {}", i18n::text(status)),
    );
    for (owner, tag, label) in [
        (menu, SYSTEM, "系统代理"),
        (menu, TUN, "TUN 模式"),
        (&state.route_menu, ROUTE_SETTINGS, "打开路由设置…"),
        (menu, REFRESH, "刷新状态"),
        (menu, START, "启动代理"),
        (menu, STOP, "停止代理"),
        (menu, OPEN, "打开主窗口"),
        (menu, QUIT, "退出 Serylane"),
    ] {
        title(&row(owner, tag), label);
    }
    refresh_traffic(target);

    let system = row(menu, SYSTEM);
    checked(&system, view.system_proxy);
    system.setEnabled(!busy && view.can_switch && shown.system_proxy == view.system_proxy);

    let tun = row(menu, TUN);
    checked(&tun, view.tun);
    tun.setEnabled(!busy && view.can_switch && view.tun_supported && shown.tun == view.tun);
    tun.setToolTip(
        (!view.tun_hint.is_empty())
            .then(|| NSString::from_str(&i18n::text(&view.tun_hint)))
            .as_deref(),
    );

    let route_item = row(menu, ROUTE);
    title(
        &route_item,
        route.map_or("路由 · 待核对", |r| {
            if r.can_toggle {
                r.label
            } else {
                "路由 · 待配置"
            }
        }),
    );
    checked(&route_item, route.is_some_and(|r| r.active));
    route_item.setEnabled(true);
    let confirm = row(&state.route_menu, ROUTE_CONFIRM);
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
    title(&row(&state.route_menu, ROUTE_INFO), &info);
    title(
        &confirm,
        if shown.route.is_some_and(|r| !r.enabled) {
            "确认关闭路由"
        } else {
            "确认开启路由"
        },
    );
    confirm.setEnabled(!busy && matching.is_some());
    row(menu, REFRESH).setEnabled(!busy);
    row(menu, START).setEnabled(!busy && view.can_start);
    row(menu, STOP).setEnabled(!busy && view.can_stop);
    let detail = row(menu, DETAIL);
    let localized = i18n::text(&view.detail);
    let message = localized.trim();
    // Preserve status/error details without opening the main window or making
    // a single long error force an excessively wide system menu.
    let mut shortened = message
        .chars()
        .take(45)
        .collect::<String>()
        .replace('\n', " ");
    if message.chars().nth(45).is_some() {
        shortened.push('…');
    }
    title(&detail, &shortened);
    detail.setHidden(message.is_empty());
    detail.setToolTip(
        (!message.is_empty())
            .then(|| NSString::from_str(message))
            .as_deref(),
    );
}

fn refresh_traffic(target: &Target) {
    let state = target.ivars();
    let current = state.traffic.stringValue();
    let item = row(&state.menu, TRAFFIC);
    if *item.title() != *current {
        title(&item, &current.to_string());
    }
}

struct NativeStatus {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
    brand: Retained<NSImage>,
    target: Retained<Target>,
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
    let state = MenuState {
        menu: NSMenu::new(mtm),
        route_menu: NSMenu::new(mtm),
        traffic: NSTextField::labelWithString(
            &NSString::from_str(if traffic_enabled {
                "正在采集流量…"
            } else {
                "流量显示已关闭"
            }),
            mtm,
        ),
        controls: RefCell::new(Controls::default()),
        presented: Cell::new(Presented::default()),
        queued: Cell::new(false),
        on_action: Arc::new(on_action),
        alive: Arc::new(AtomicBool::new(true)),
    };
    let target: Retained<Target> =
        unsafe { msg_send![super(Target::alloc(mtm).set_ivars(state)), init] };
    populate(&target);
    let menu = &target.ivars().menu;
    menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    button.setImage(Some(&brand));
    button.setToolTip(Some(&NSString::from_str("Serylane")));
    button.setAccessibilityLabel(Some(&NSString::from_str(&i18n::text("Serylane 快捷菜单"))));
    item.setMenu(Some(menu));
    button.sendActionOn(NSEventMask::LeftMouseDown.union(NSEventMask::RightMouseDown));
    STATUS.with_borrow_mut(|status| {
        *status = Some(NativeStatus {
            bar,
            item,
            brand,
            target,
        })
    });
    Ok(())
}

pub(super) fn refresh_locale(marker: MainThreadMarker) {
    let entry = STATUS.with_borrow(|owned| {
        owned
            .as_ref()
            .map(|status| (status.item.clone(), status.target.clone()))
    });
    if let Some((item, target)) = entry {
        if let Some(button) = item.button(marker) {
            button
                .setAccessibilityLabel(Some(&NSString::from_str(&i18n::text("Serylane 快捷菜单"))));
        }
        render(&target, !is_visible());
    }
}

pub(super) fn is_visible() -> bool {
    MENU_VISIBLE.load(Ordering::Acquire)
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
        state.menu.cancelTracking();
        state.menu.setDelegate(None);
        owned.item.setMenu(None);
        owned.bar.removeStatusItem(&owned.item);
    }
    MENU_VISIBLE.store(false, Ordering::Release);
}
