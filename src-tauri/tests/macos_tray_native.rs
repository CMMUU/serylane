// AppKit must run on the process main thread, not a Rust test worker. This
// harness tests the production title and quick panel with disposable status items;
// it never opens Serylane, reads user configuration or starts networking.
#[cfg(target_os = "macos")]
#[path = "../src/traffic_monitor/macos_layout.rs"]
#[allow(dead_code)]
mod macos_layout;
#[cfg(target_os = "macos")]
#[path = "../src/traffic_monitor/macos_title.rs"]
#[allow(dead_code)]
mod macos_title;

#[cfg(target_os = "macos")]
#[path = "../src/macos_menu_panel.rs"]
mod macos_menu_panel;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSApplication, NSApplicationActivationPolicy, NSAttributedStringNSExtendedStringDrawing,
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSMenu, NSStatusBar,
        NSStringDrawingOptions,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSSize};

    let mtm = MainThreadMarker::new().expect("native test main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if let Some(folder) = std::env::var_os("SERYLANE_REALSPACE_QA_DIR") {
        real_space_host(&app, mtm, std::path::Path::new(&folder));
        return;
    }
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(macos_layout::item_width());
    let button = item.button(mtm).expect("disposable status button");
    let menu = NSMenu::new(mtm);
    item.setMenu(Some(&menu));
    let rgba = macos_layout::brand_icon();
    let icon = image_from_rgba(rgba);
    assert_eq!(icon.size(), NSSize::new(12.0, 18.0));
    icon.setTemplate(true);
    button.setImage(Some(&icon));
    let mut text_origin = None;
    for (upload, download) in [
        (0, 0),
        (5939, 6451),
        (7_987, 137_216),
        (10_188, 10_189),
        (999 * 1024, 1023 * 1024),
        (1023, 1024),
        (1023 * 1024, 12 * 1024 * 1024),
        (1 << 40, u64::MAX),
    ] {
        macos_title::apply_to_item(&item, upload, download, mtm).unwrap();
        assert_eq!(button.image().unwrap().size(), NSSize::new(12.0, 18.0));
        assert_eq!(button.font().unwrap().pointSize(), 9.5);
        let image_rect = button.cell().unwrap().imageRectForBounds(button.bounds());
        assert!(image_rect.origin.x >= 0.0);
        assert!(image_rect.origin.x + image_rect.size.width <= button.bounds().size.width);
        assert!(
            image_rect.origin.x <= 6.0,
            "excess horizontal image inset: {image_rect:?}"
        );
        assert_eq!(item.length(), macos_layout::item_width());
        assert!(
            std::ptr::eq(&*item.menu(mtm).unwrap(), &*menu),
            "layout must retain menu ownership"
        );
        let frame = button.frame();
        for x in [1.0, frame.size.width - 1.0] {
            assert!(
                button
                    .hitTest(NSPoint::new(
                        frame.origin.x + x,
                        frame.origin.y + frame.size.height / 2.0
                    ))
                    .is_some(),
                "compact button edge must stay clickable"
            );
        }
        let title = button.attributedTitle();
        assert_eq!(title.string().to_string().lines().count(), 2);
        assert_eq!(title.string().to_string().matches('\t').count(), 6);
        assert!(!button.cell().unwrap().usesSingleLineMode());
        assert_eq!(button.attributedTitle().length(), title.length());
        let bounds = macos_title::build_title(upload, download)
            .boundingRectWithSize_options_context(
                NSSize::new(48.0, 100.0),
                NSStringDrawingOptions::UsesLineFragmentOrigin
                    | NSStringDrawingOptions::UsesFontLeading,
                None,
            );
        assert_eq!(bounds.size.height, macos_layout::LINE_HEIGHT * 2.0);
        assert!(
            bounds.size.width <= 42.0,
            "native columns overflow: {bounds:?}"
        );
        assert!(
            bounds.size.width
                <= button
                    .cell()
                    .unwrap()
                    .titleRectForBounds(button.bounds())
                    .size
                    .width
        );
        let origin = button
            .cell()
            .unwrap()
            .titleRectForBounds(button.bounds())
            .origin
            .x;
        assert_eq!(
            origin,
            *text_origin.get_or_insert(origin),
            "unit changes must not shift native columns"
        );
    }
    macos_title::clear_item(&item, mtm).unwrap();
    assert_eq!(button.attributedTitle().length(), 0);
    macos_title::apply_to_item(&item, 5939, 6451, mtm).unwrap();
    assert_eq!(item.length(), macos_layout::item_width());
    println!(
        "native button: titleRect={:?}, font={:?}",
        button.cell().unwrap().titleRectForBounds(button.bounds()),
        button.font().map(|font| font.pointSize())
    );
    let folder =
        std::env::var_os("SERYLANE_NATIVE_TRAY_SNAPSHOT_DIR").map(std::path::PathBuf::from);
    if let Some(folder) = &folder {
        std::fs::create_dir_all(folder).unwrap();
    }
    // Always render in CI too: a string-height assertion alone missed the
    // NSStatusBarButton single-line-baseline clipping regression.
    for scale in [1, 2] {
        for (name, dark, highlighted) in [
            ("light", false, false),
            ("dark", true, false),
            ("selected", false, true),
        ] {
            let appearance = unsafe {
                NSAppearance::appearanceNamed(if dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                })
            }
            .unwrap();
            button.setAppearance(Some(&appearance));
            button.cell().unwrap().setHighlighted(highlighted);
            let rect = button.bounds();
            let bitmap = unsafe {
                NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                    NSBitmapImageRep::alloc(), std::ptr::null_mut(),
                    rect.size.width as isize * scale, rect.size.height as isize * scale,
                    8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
                )
            }.unwrap();
            bitmap.setSize(rect.size);
            // NSBitmapImageRep owns uninitialized storage. cacheDisplay uses
            // compositing and does not clear it (verified with a nonzero
            // control canvas). Clear every plane before checking edge ink;
            // otherwise allocator residue looks like clipping on macOS 14/15.
            unsafe {
                std::ptr::write_bytes(bitmap.bitmapData(), 0, bitmap.bytesPerPlane() as usize);
            }
            button.cacheDisplayInRect_toBitmapImageRep(rect, &bitmap);
            if let Some(folder) = &folder {
                let png = unsafe {
                    bitmap.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }
                .unwrap();
                std::fs::write(
                    folder.join(format!("native-{name}-{scale}x.png")),
                    png.to_vec(),
                )
                .unwrap();
            }
            if !highlighted {
                let title_rect = button.cell().unwrap().titleRectForBounds(rect);
                let intrinsic_width = macos_title::build_title(5939, 6451)
                    .boundingRectWithSize_options_context(
                        NSSize::new(48.0, 100.0),
                        NSStringDrawingOptions::UsesLineFragmentOrigin,
                        None,
                    )
                    .size
                    .width;
                // macOS 14 reports an expanded 60pt title rectangle for the
                // centered 42pt text. Scanning its raw left edge includes the
                // right half of S, falsely joining the two text ink bands.
                // macOS 15/current return the intrinsic rectangle already.
                let text_x = ((title_rect.origin.x
                    + (title_rect.size.width - intrinsic_width).max(0.0) / 2.0)
                    * scale as f64)
                    .ceil() as isize;
                let rows: Vec<bool> = (0..bitmap.pixelsHigh())
                    .map(|y| {
                        (text_x..bitmap.pixelsWide())
                            // Offscreen light status buttons carry system
                            // translucency; test ink coverage, not opacity.
                            .any(|x| bitmap.colorAtX_y(x, y).unwrap().alphaComponent() > 0.10)
                    })
                    .collect();
                let runs = rows
                    .iter()
                    .enumerate()
                    .filter(|(y, ink)| **ink && (*y == 0 || !rows[y - 1]))
                    .count();
                if runs != 2 || rows[0] || rows[rows.len() - 1] {
                    println!(
                        "ink rows {name} {scale}x: {:?}",
                        rows.iter()
                            .enumerate()
                            .filter(|(_, ink)| **ink)
                            .map(|(y, _)| y)
                            .collect::<Vec<_>>()
                    );
                    println!(
                        "native geometry: flipped={}, imageRect={:?}, titleRect={:?}",
                        button.isFlipped(),
                        button.cell().unwrap().imageRectForBounds(rect),
                        button.cell().unwrap().titleRectForBounds(rect)
                    );
                }
                assert_eq!(
                    runs, 2,
                    "{name} at {scale}x: both rows must be fully visible"
                );
                assert!(
                    !rows[0] && !rows[rows.len() - 1],
                    "{name} at {scale}x: text touches clipping edge"
                );
            }
            println!(
                "native snapshot {name}: bounds={rect:?}, pixels={}x{}",
                bitmap.pixelsWide(),
                bitmap.pixelsHigh()
            );
        }
    }
    // The bitmap path restores the same slot, without tray-icon's 18pt shrink.
    macos_title::clear_item(&item, mtm).unwrap();
    let (pixels, width, height) = macos_layout::bitmap_canvas();
    let bitmap_image = image_from_rgba(&tauri::image::Image::new_owned(pixels, width, height));
    bitmap_image.setTemplate(true);
    button.setImage(Some(&bitmap_image));
    macos_title::apply_bitmap_to_item(&item, mtm).unwrap();
    assert_eq!(item.length(), 64.0);
    assert!(std::ptr::eq(&*item.menu(mtm).unwrap(), &*menu));
    assert_eq!(button.image().unwrap().size(), NSSize::new(56.0, 22.0));
    let image_rect = button.cell().unwrap().imageRectForBounds(button.bounds());
    assert!(image_rect.origin.x >= 0.0 && image_rect.origin.x <= 5.0);
    assert!(image_rect.origin.x + image_rect.size.width <= button.bounds().size.width);
    macos_title::clear_item(&item, mtm).unwrap();
    button.setImage(Some(&icon));
    macos_title::apply_to_item(&item, 7_987, 137_216, mtm).unwrap();
    assert_eq!(item.length(), 64.0);
    assert_eq!(button.image().unwrap().size(), NSSize::new(12.0, 18.0));
    assert_eq!(
        button
            .attributedTitle()
            .string()
            .to_string()
            .lines()
            .count(),
        2
    );
    bar.removeStatusItem(&item);
    println!("native tray: fixed-width two-row title, unit changes, clear and re-enable passed");
    check_panel(&app, mtm);
}

#[cfg(target_os = "macos")]
fn check_panel(app: &objc2_app_kit::NSApplication, mtm: objc2::MainThreadMarker) {
    use macos_menu_panel::Action;
    use objc2::{rc::Retained, sel};
    use objc2_app_kit::{
        NSButton, NSEventMask, NSPopUpMenuWindowLevel, NSStandardKeyBindingResponding,
        NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSRunLoop, NSSize};
    use std::sync::{Arc, Mutex};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }
    let reference = rect(-1440.0, 138.0, 1440.0, 850.0);
    let panel_size = macos_menu_panel::SIZE;
    assert_eq!(
        macos_menu_panel::panel_origin(panel_size, rect(-280.0, 985.0, 64.0, 24.0), reference),
        NSPoint::new(-400.0, 607.0)
    );
    assert_eq!(
        macos_menu_panel::panel_origin(panel_size, rect(-1440.0, 1080.0, 40.0, 24.0), reference),
        NSPoint::new(-1432.0, 610.0)
    );
    assert_eq!(
        macos_menu_panel::panel_origin(panel_size, rect(-27.0, 140.0, 25.0, 24.0), reference),
        NSPoint::new(-312.0, 146.0)
    );
    let square = rect(-300.0, 20.0, 30.0, 30.0);
    let icon_frame = rect(-285.0, 70.0, 20.0, 15.0);
    assert!(macos_menu_panel::outside_frames(
        NSPoint::new(-299.0, 21.0),
        square,
        Some(icon_frame)
    ));
    assert!(!macos_menu_panel::outside_frames(
        NSPoint::new(-285.0, 35.0),
        square,
        Some(icon_frame)
    ));
    assert!(!macos_menu_panel::outside_frames(
        NSPoint::new(-280.0, 71.0),
        square,
        Some(icon_frame)
    ));
    assert!(macos_menu_panel::outside_frames(
        NSPoint::new(-280.0, 63.0),
        square,
        Some(icon_frame)
    ));

    app.finishLaunching();
    let actions = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&actions);
    let brand = || macos_title::status_image(macos_layout::brand_icon(), true).unwrap();
    macos_menu_panel::install(mtm, brand(), false, move |action| {
        observed.lock().unwrap().push(action);
    })
    .unwrap();
    assert!(macos_menu_panel::install(mtm, brand(), true, |_| {}).is_err());

    let status = macos_menu_panel::with_item(mtm, |item, brand, traffic| {
        let button = item.button(mtm).unwrap();
        assert!(
            item.menu(mtm).is_none(),
            "native panel and old menu cannot coexist"
        );
        assert!(button.image().unwrap().isTemplate());
        assert_eq!(button.image().unwrap().size(), brand.size());
        assert_eq!(traffic.stringValue().to_string(), "流量显示已关闭");
        assert_eq!(
            button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown),
            (NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown).bits() as isize
        );
        // Rendering a new sample does not replace the AppKit target or action.
        macos_title::apply_to_item(item, 8_192, 16_384, mtm).unwrap();
        assert_eq!(button.action(), Some(sel!(toggleSerylanePanel:)));
        macos_title::clear_item(item, mtm).unwrap();
        assert_eq!(button.action(), Some(sel!(toggleSerylanePanel:)));
        button
    })
    .expect("status registered");
    let panel = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == "Serylane 快捷面板")
        .expect("panel window registered");
    assert!(panel
        .styleMask()
        .contains(NSWindowStyleMask::NonactivatingPanel));
    assert!(panel.collectionBehavior().contains(
        NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::CanJoinAllApplications
            | NSWindowCollectionBehavior::CanJoinAllSpaces
    ));
    assert_eq!(panel.level(), NSPopUpMenuWindowLevel);
    assert!(panel.canBecomeKeyWindow() && !panel.canBecomeMainWindow());
    assert!(!panel.isVisible());
    let click_status = || {
        // Dispatch through the real button and target; this is not a physical click.
        assert!(unsafe { status.sendAction_to(status.action(), status.target().as_deref()) });
    };
    let foreground = || {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    };
    use macos_menu_panel::{update_controls, Controls, RouteRow, RouteToken};
    let token = RouteToken {
        enabled: true,
        revision: 11,
        profile: None,
        profile_revision: None,
    };
    let system_on = Controls {
        serial: 1,
        status: "系统代理运行中",
        can_switch: true,
        can_start: false,
        can_stop: true,
        tun_supported: true,
        system_proxy: true,
        route: Some(RouteRow {
            token,
            label: "路由",
            message: "已关闭".into(),
            can_toggle: true,
            active: false,
            domains: 4,
            target: "QA".into(),
        }),
        ..Controls::default()
    };
    update_controls(mtm, system_on.clone());

    let pump = || {
        let until = NSDate::dateWithTimeIntervalSinceNow(0.12);
        let run_loop = NSRunLoop::currentRunLoop();
        while until.timeIntervalSinceNow() > 0.0 {
            run_loop.runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &until);
        }
    };
    let take = || {
        pump();
        std::mem::take(&mut *actions.lock().unwrap())
    };
    let initial_foreground = foreground();
    click_status();
    assert!(panel.isVisible() && macos_menu_panel::is_visible());
    assert_eq!(
        foreground(),
        initial_foreground,
        "opening must preserve foreground"
    );
    assert!(panel.isOnActiveSpace());
    assert_eq!(take(), [Action::Observe]);

    let inside = NSPoint::new(
        panel.frame().origin.x + 40.0,
        panel.frame().origin.y + 170.0,
    );
    macos_menu_panel::dismiss_if_outside(mtm, inside);
    assert!(panel.isVisible(), "inner click must not close the panel");
    let outside = NSPoint::new(panel.frame().origin.x + 40.0, panel.frame().origin.y - 6.0);
    macos_menu_panel::dismiss_if_outside(mtm, outside);
    assert!(!panel.isVisible() && !macos_menu_panel::is_visible());
    click_status();
    assert_eq!(take(), [Action::Observe]);
    click_status();
    assert!(!panel.isVisible(), "same icon toggles off");
    pump(); // allow AppKit to finish the prior status button's tracking cycle
    click_status();
    assert!(panel.isVisible());
    assert_eq!(take(), [Action::Observe]);
    unsafe { panel.cancelOperation(None) };
    assert!(!panel.isVisible(), "Escape closes");

    click_status();
    assert_eq!(take(), [Action::Observe]);
    let surface = panel.contentView().expect("native content view");
    assert!(!panel.isOpaque() && panel.hasShadow());
    let layer = surface.layer().expect("blur layer");
    assert!(
        layer.masksToBounds(),
        "one mask clips the material and its border"
    );
    let radius: f64 = unsafe { objc2::msg_send![&*layer, cornerRadius] };
    assert_eq!(radius, 14.0);
    let mut buttons: Vec<Retained<NSButton>> = surface
        .subviews()
        .iter()
        .filter_map(|view| view.downcast().ok())
        .collect();
    buttons.sort_by_key(|b| b.tag());
    assert_eq!(buttons.len(), 8);
    assert!(buttons
        .windows(2)
        .all(|pair| pair[0].frame().origin.y > pair[1].frame().origin.y));
    let press = |index: usize| {
        let button = &buttons[index];
        let _ = unsafe { button.sendAction_to(button.action(), button.target().as_deref()) };
    };
    assert!(
        buttons[0].title().to_string().starts_with('✓')
            && !buttons[4].isEnabled()
            && buttons[5].isEnabled()
    );
    press(0);
    press(0);
    assert_eq!(take(), [Action::SystemProxy(false)]);
    assert!(panel.isVisible(), "leave controls visible for feedback");
    assert!(
        !buttons[0].isEnabled(),
        "queued action locally locks duplicates"
    );
    update_controls(mtm, system_on.clone()); // duplicate observation cannot unlock queued click
    update_controls(
        mtm,
        Controls {
            serial: 2,
            ..system_on.clone()
        },
    );
    assert!(
        !buttons[0].isEnabled(),
        "new quiet observation cannot unlock queued click"
    );
    update_controls(
        mtm,
        Controls {
            serial: 3,
            action_serial: 3,
            busy: Some("stopping"),
            ..system_on.clone()
        },
    );
    press(3);
    assert!(
        take().is_empty(),
        "reject actions until the shared operation completes"
    );

    let stopped = Controls {
        serial: 4,
        action_serial: 4,
        status: "已关闭",
        can_start: true,
        can_stop: false,
        system_proxy: false,
        ..system_on.clone()
    };
    update_controls(mtm, stopped.clone());
    update_controls(mtm, system_on.clone()); // delayed IPC read
    assert!(!buttons[0].title().to_string().starts_with('✓') && buttons[4].isEnabled());
    press(0);
    assert_eq!(take(), [Action::SystemProxy(true)]);
    let update = |serial| {
        update_controls(
            mtm,
            Controls {
                serial,
                action_serial: serial,
                ..stopped.clone()
            },
        )
    };
    update(5);
    press(1);
    assert_eq!(take(), [Action::Tun(true)]);
    update(6);
    press(4);
    assert_eq!(take(), [Action::Start]);
    update(7);
    press(3);
    assert_eq!(take(), [Action::Refresh]);
    update(8);

    press(2);
    assert!(buttons[2].title().to_string().contains("确认开启路由") && take().is_empty());
    press(3);
    assert!(take().is_empty() && !buttons[2].title().to_string().contains("确认"));
    press(2);
    press(2);
    press(2);
    assert_eq!(take(), [Action::Route(token)]);
    assert!(panel.isVisible());
    update_controls(
        mtm,
        Controls {
            serial: 9,
            action_serial: 9,
            ..system_on.clone()
        },
    );
    press(5);
    assert_eq!(take(), [Action::Stop]);
    assert!(panel.isVisible());
    assert!(!buttons[5].isEnabled());
    macos_menu_panel::reject_queued(mtm, 8, "old rejection".into());
    assert!(!buttons[5].isEnabled(), "ignore a stale rejection");
    macos_menu_panel::reject_queued(mtm, 9, "fixture rejection".into());
    assert!(
        buttons[5].isEnabled(),
        "a non-admitted action releases the latch"
    );
    update(10);
    assert!(buttons[7].title().to_string().ends_with("退出 Serylane"));
    for (index, action) in [(6, Action::OpenWindow), (7, Action::Quit)] {
        if !panel.isVisible() {
            click_status();
            assert_eq!(take(), [Action::Observe]);
        }
        press(index);
        assert!(!panel.isVisible());
        assert_eq!(take(), [action]);
    }

    click_status();
    macos_menu_panel::uninstall(mtm);
    assert!(!panel.isVisible() && status.target().is_none() && status.action().is_none());
    assert!(macos_menu_panel::with_item(mtm, |_, _, _| ()).is_none());
    macos_menu_panel::uninstall(mtm);
    println!("native panel: rounded layer, ordered rows, original anchor and mouse masks, outside/Escape and repeat toggle, nonactivation, shared-state rendering, all controls, confirmation and exactly one deferred action passed; hardware full-screen acceptance is separate");
}

/// Opt-in isolated WindowServer host. The external QA companion posts actual
/// mouse/key events; neither fixture reads product settings or runs a proxy.
#[cfg(target_os = "macos")]
fn real_space_host(
    app: &objc2_app_kit::NSApplication,
    mtm: objc2::MainThreadMarker,
    folder: &std::path::Path,
) {
    use macos_menu_panel::{Controls, RouteRow, RouteToken};
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSAccessibility, NSButton, NSEventMask, NSScreen, NSWorkspace};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRect, NSString};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    app.finishLaunching();
    let actions = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&actions);
    // A unique color in the fixture's otherwise production S artwork makes
    // the GUI driver locate the real icon, not click a stale calendar/overflow
    // position when the system folds status items around the notch.
    let source = macos_layout::brand_icon();
    let mut pixels = source.rgba().to_vec();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[..3].copy_from_slice(&[245, 17, 157]);
    }
    let brand = image_from_rgba(&tauri::image::Image::new_owned(
        pixels,
        source.width(),
        source.height(),
    ));
    macos_menu_panel::install(mtm, brand, true, move |action| {
        observed.lock().unwrap().push(format!("{action:?}"));
    })
    .unwrap();
    let button = macos_menu_panel::with_item(mtm, |item, brand, traffic| {
        let button = item.button(mtm).unwrap();
        button.setImage(Some(brand));
        button.setToolTip(Some(&NSString::from_str("Serylane isolated QA")));
        button.setAccessibilityLabel(Some(&NSString::from_str("Serylane 隔离验收菜单")));
        traffic.setStringValue(&NSString::from_str("↑ 8.0 KiB/s    ↓ 16.0 KiB/s"));
        button
    })
    .unwrap();
    let panel = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == "Serylane 快捷面板")
        .unwrap();
    let content = panel.contentView().unwrap();
    let rows: Vec<_> = content
        .subviews()
        .iter()
        .filter_map(|view| view.downcast::<NSButton>().ok())
        .collect();

    let mut confirmed = 0;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 150 && !folder.join("stop").exists() {
        autoreleasepool(|_| {
            // Pull events through NSApplication, not through direct selector
            // dispatch; both status tracking and local monitors stay active.
            let until = NSDate::dateWithTimeIntervalSinceNow(0.04);
            while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                unsafe { NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&event);
                if until.timeIntervalSinceNow() <= 0.0 {
                    break;
                }
            }
            let actions = actions.lock().unwrap();
            let writes = actions
                .iter()
                .filter(|action| action.as_str() != "Observe")
                .count();
            if writes + 1 > confirmed {
                confirmed = writes + 1;
                macos_menu_panel::update_controls(
                    mtm,
                    Controls {
                        serial: confirmed as u64,
                        action_serial: writes as u64,
                        status: if writes == 0 {
                            "已关闭"
                        } else {
                            "系统代理运行中"
                        },
                        can_switch: true,
                        can_start: writes == 0,
                        can_stop: writes != 0,
                        system_proxy: writes != 0,
                        tun_supported: true,
                        route: Some(RouteRow {
                            token: RouteToken {
                                enabled: true,
                                revision: 1,
                                profile: None,
                                profile_revision: None,
                            },
                            label: "路由",
                            message: "已关闭".into(),
                            can_toggle: true,
                            active: false,
                            domains: 4,
                            target: "QA".into(),
                        }),
                        ..Controls::default()
                    },
                );
            }
            let screens = NSScreen::screens(mtm);
            let screen = screens.firstObject().expect("GUI screen");
            let to_cg = |r: NSRect| {
                [
                    r.origin.x,
                    screen.frame().size.height - r.origin.y - r.size.height,
                    r.size.width,
                    r.size.height,
                ]
            };
            let anchor = button.window().map(|w| {
                to_cg(w.convertRectToScreen(button.convertRect_toView(button.bounds(), None)))
            });
            let boxes: Vec<_> = rows
                .iter()
                .map(|b| to_cg(panel.convertRectToScreen(b.convertRect_toView(b.bounds(), None))))
                .collect();
            let front = NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|app| app.processIdentifier());
            let status_window = button.window();
            let data = json!({
                "statusWindow": status_window.as_ref().map(|window| window.windowNumber()),
                "statusVisible": status_window.as_ref().is_some_and(|window| window.isVisible()),
                "pid": std::process::id(), "front": front, "screens": screens.count(), "anchor": anchor,
                "panel": to_cg(panel.frame()), "rows": boxes, "visible": panel.isVisible(),
                "activeSpace": panel.isOnActiveSpace(), "window": panel.windowNumber(),
                "actions": &*actions,
            });
            let temp = folder.join("panel.tmp");
            std::fs::write(&temp, data.to_string()).unwrap();
            std::fs::rename(temp, folder.join("panel.json")).unwrap();
        });
    }
    macos_menu_panel::uninstall(mtm);
}

#[cfg(target_os = "macos")]
fn image_from_rgba(rgba: &tauri::image::Image<'_>) -> objc2::rc::Retained<objc2_app_kit::NSImage> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage};
    use objc2_foundation::NSSize;
    let rep = unsafe { NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
        NSBitmapImageRep::alloc(), std::ptr::null_mut(), rgba.width() as isize, rgba.height() as isize,
        8, 4, true, false, NSDeviceRGBColorSpace, (rgba.width() * 4) as isize, 32,
    ) }.unwrap();
    unsafe {
        std::ptr::copy_nonoverlapping(rgba.rgba().as_ptr(), rep.bitmapData(), rgba.rgba().len());
    }
    let size = NSSize::new(rgba.width() as f64 / 2.0, rgba.height() as f64 / 2.0);
    rep.setSize(size);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);
    image
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native macOS tray integration is not applicable on this platform");
}
