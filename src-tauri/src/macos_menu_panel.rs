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
    NSPanel, NSPopUpMenuWindowLevel, NSScreen, NSStatusBar, NSStatusBarButton, NSStatusItem,
    NSTextField, NSVariableStatusItemLength, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
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

const SIZE: NSSize = NSSize::new(340.0, 158.0);
const INSET: f64 = 8.0;
const ACTION_EVENTS: NSEventMask = NSEventMask::LeftMouseDown.union(NSEventMask::RightMouseDown);
const NOT_SAMPLED: &str = "正在采集流量…";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    OpenWindow = 0,
    Stop = 1,
    Quit = 2,
}

define_class!(
    #[unsafe(super = NSPanel)]
    #[name = "SerylaneQuickPanel"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<bool>]
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
        let this = Self::alloc(mtm).set_ivars(Cell::new(false));
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
        self.setFrameOrigin(panel_origin(
            self.frame().size,
            anchor,
            display.visibleFrame(),
        ));
        self.ivars().set(true);
        self.makeKeyAndOrderFront(None);
    }

    fn is_open(&self) -> bool {
        self.ivars().get() && self.isVisible()
    }

    fn dismiss(&self) {
        // orderOut can deliver windowDidResignKey synchronously.
        if self.ivars().replace(false) {
            self.orderOut(None);
        }
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

type ActionHandler = Arc<dyn Fn(Action) + Send + Sync>;

struct PanelState {
    panel: Retained<QuickPanel>,
    status_button: Retained<NSStatusBarButton>,
    traffic: Retained<NSTextField>,
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
            let panel = &self.ivars().panel;
            if panel.is_open() {
                panel.dismiss();
            } else if let Some((anchor, screen)) = button_anchor(sender) {
                panel.open_at(anchor, &screen);
            }
        }

        #[unsafe(method(serylanePanelAction:))]
        fn select_action(&self, sender: &NSButton) {
            let state = self.ivars();
            if !state.panel.is_open() {
                return;
            }
            let action = match sender.tag() {
                0 => Action::OpenWindow,
                1 => Action::Stop,
                2 => Action::Quit,
                _ => return,
            };
            state.panel.dismiss();
            let handler = Arc::clone(&state.on_action);
            let alive = Arc::clone(&state.alive);
            let block = RcBlock::new(move || {
                if alive.load(Ordering::Acquire) {
                    handler(action);
                }
            });
            // SAFETY: the block's captures are Send + Sync; the queue is the
            // main queue. Do not activate/stop/exit inside AppKit event delivery.
            unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
        }
    }
);

fn populate_panel(target: &Target, mtm: MainThreadMarker) {
    let panel = &target.ivars().panel;
    let surface = NSVisualEffectView::initWithFrame(
        NSVisualEffectView::alloc(mtm),
        rect(0.0, 0.0, SIZE.width, SIZE.height),
    );
    surface.setMaterial(NSVisualEffectMaterial::Popover);
    surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    surface.setState(NSVisualEffectState::Active);
    surface.setWantsLayer(true);
    if let Some(layer) = surface.layer() {
        // SAFETY: AppKit supplies a CALayer; its public CGFloat setter
        // uses the double ABI on both supported 64-bit macOS architectures.
        unsafe {
            let _: () = msg_send![&*layer, setCornerRadius: 14.0_f64];
        }
        layer.setMasksToBounds(true);
    }
    let heading = NSTextField::labelWithString(&NSString::from_str("Serylane"), mtm);
    let caption = NSTextField::labelWithString(&NSString::from_str("全局流量"), mtm);
    // SAFETY: AppKit system-font constructors accept these positive sizes and weights.
    unsafe {
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(
            17.0,
            NSFontWeightSemibold,
        )));
        target
            .ivars()
            .traffic
            .setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                13.0,
                NSFontWeightMedium,
            )));
    }
    caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
    heading.setFrame(rect(18.0, 119.0, 304.0, 23.0));
    caption.setFrame(rect(18.0, 93.0, 304.0, 17.0));
    target
        .ivars()
        .traffic
        .setFrame(rect(18.0, 68.0, 304.0, 21.0));
    surface.addSubview(&heading);
    surface.addSubview(&caption);
    surface.addSubview(&target.ivars().traffic);

    panel.setContentView(Some(&surface));
    let mut previous = None::<Retained<NSButton>>;
    for (action, title, x, width) in [
        (Action::OpenWindow, "打开主窗口", 16.0, 108.0),
        (Action::Stop, "停止代理", 128.0, 90.0),
        (Action::Quit, "退出 Serylane", 222.0, 102.0),
    ] {
        // SAFETY: Target implements serylanePanelAction: for an NSButton.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(target),
                Some(sel!(serylanePanelAction:)),
                mtm,
            )
        };
        button.setTag(action as isize);
        button.setFrame(rect(x, 19.0, width, 36.0));
        surface.addSubview(&button);
        if let Some(prev) = previous {
            // SAFETY: both views belong to this panel and share its lifetime.
            unsafe { prev.setNextKeyView(Some(&button)) };
        } else {
            panel.setInitialFirstResponder(Some(&button));
        }
        previous = Some(button);
    }
    if let Some(last) = previous {
        // SAFETY: the first responder is a button retained by the same view.
        unsafe { last.setNextKeyView(panel.initialFirstResponder().as_deref()) };
    }
    panel.setDelegate(Some(ProtocolObject::from_ref(target)));
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
