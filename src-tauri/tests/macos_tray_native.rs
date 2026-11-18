// AppKit must run on the process main thread, not a Rust test worker. This
// harness tests the production title and NSMenu with disposable status items;
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
    check_menu(mtm);
}

#[cfg(target_os = "macos")]
fn check_menu(mtm: objc2::MainThreadMarker) {
    use block2::RcBlock;
    use macos_menu_panel::*;
    use objc2_app_kit::{
        NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuDelegate, NSMenuItem,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSOperationQueue, NSRunLoop, NSString};
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    let actions = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&actions);
    install(
        mtm,
        image_from_rgba(macos_layout::brand_icon()),
        true,
        move |action| recorded.lock().unwrap().push(action),
    )
    .unwrap();
    assert!(install(
        mtm,
        image_from_rgba(macos_layout::brand_icon()),
        true,
        |_| {}
    )
    .is_err());

    let menu = with_item(mtm, |item, _, traffic| {
        traffic.setStringValue(&NSString::from_str("↑ 8.0 KiB/s    ↓ 16.0 KiB/s"));
        let menu = item.menu(mtm).expect("actual AppKit status menu");
        macos_title::apply_to_item(item, 1200, 6500, mtm).unwrap();
        assert!(std::ptr::eq(&*item.menu(mtm).unwrap(), &*menu));
        menu
    })
    .unwrap();
    let row = |tag| menu.itemWithTag(tag).unwrap();
    let route = row(ROUTE).submenu().unwrap();
    let confirm = route.itemWithTag(ROUTE_CONFIRM).unwrap();
    let delegate = menu.delegate().unwrap();
    assert!(!menu.autoenablesItems());
    assert!(!route.autoenablesItems());
    assert!(row(SYSTEM).view().is_none(), "use a real AppKit menu row");
    assert_eq!(
        menu.itemArray()
            .iter()
            .filter(|r| r.isSeparatorItem())
            .count(),
        3
    );
    assert!(row(TRAFFIC).title().to_string().contains("8.0 KiB/s"));
    assert!(!row(TUN).isEnabled());

    let token = RouteToken {
        enabled: true,
        revision: 7,
        profile: None,
        profile_revision: None,
    };
    let view = Controls {
        serial: 1,
        action_serial: 0,
        status: "系统代理运行中",
        system_proxy: true,
        can_start: false,
        can_stop: true,
        can_switch: true,
        tun_supported: true,
        route: Some(RouteRow {
            token,
            label: "路由 · 待启动",
            message: "已开启".into(),
            can_toggle: true,
            active: false,
            domains: 2,
            target: "QA".into(),
        }),
        ..Controls::default()
    };
    update_controls(mtm, view.clone());
    assert!(row(STATUS_ROW)
        .title()
        .to_string()
        .contains("系统代理运行中"));
    assert_eq!(row(SYSTEM).state(), NSControlStateValueOn);
    assert_eq!(row(TUN).state(), NSControlStateValueOff);
    assert_eq!(row(ROUTE).state(), NSControlStateValueOff);
    assert!(!row(START).isEnabled());
    assert!(row(STOP).isEnabled());
    assert!(confirm.isEnabled());

    // Drain in the same mode as AppKit dispatch. A fixed sleep is insufficient
    // on the Intel runner and clicking a disabled row must emit no action.
    let take = |expected| {
        let fence = Arc::new(AtomicBool::new(false));
        let fence_block = Arc::clone(&fence);
        let block = RcBlock::new(move || fence_block.store(true, Ordering::Release));
        // SAFETY: the captured atomic is Send + Sync.
        unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
        let deadline = Instant::now() + Duration::from_secs(4);
        while (!fence.load(Ordering::Acquire) || actions.lock().unwrap().len() < expected)
            && Instant::now() < deadline
        {
            NSRunLoop::currentRunLoop().runMode_beforeDate(
                unsafe { NSDefaultRunLoopMode },
                &NSDate::dateWithTimeIntervalSinceNow(0.015),
            );
        }
        assert!(fence.load(Ordering::Acquire), "action queue fence");
        std::mem::take(&mut *actions.lock().unwrap())
    };
    let click = |menu: &NSMenu, item: &NSMenuItem| {
        menu.performActionForItemAtIndex(menu.indexOfItem(item));
    };

    delegate.menuNeedsUpdate(&menu);
    delegate.menuWillOpen(&menu);
    assert!(is_visible());
    assert_eq!(take(1), [Action::Observe]);
    let mut changed = view.clone();
    changed.serial = 2;
    changed.system_proxy = false;
    changed.tun = true;
    changed.route.as_mut().unwrap().token.revision = 8;
    update_controls(mtm, changed.clone());
    assert_eq!(row(TUN).state(), NSControlStateValueOn);
    assert!(!row(SYSTEM).isEnabled() && !row(TUN).isEnabled() && !confirm.isEnabled());
    click(&menu, &row(TUN));
    click(&route, &confirm);
    assert!(
        take(0).is_empty(),
        "stale menu action must not reverse intent"
    );
    delegate.menuDidClose(&menu);
    assert!(!is_visible());

    delegate.menuNeedsUpdate(&menu);
    assert!(confirm.isEnabled());
    click(&route, &confirm);
    assert!(!row(STOP).isEnabled());
    click(&menu, &row(STOP));
    assert_eq!(
        take(1),
        [Action::Route(RouteToken {
            revision: 8,
            ..token
        })]
    );
    let mut quiet = changed.clone();
    quiet.serial = 3;
    update_controls(mtm, quiet);
    update_controls(mtm, view);
    assert!(
        !row(STOP).isEnabled(),
        "old read may not undo local busy lock"
    );
    reject_queued(mtm, 0, "操作未完成：隔离检验".into());
    assert!(row(STOP).isEnabled());
    assert!(row(DETAIL).title().to_string().contains("操作未完成"));

    changed.serial = 4;
    changed.action_serial = 1;
    update_controls(mtm, changed.clone());
    click(&menu, &row(TUN));
    assert_eq!(take(1), [Action::Tun(false)]);
    let mut pending = changed.clone();
    pending.serial = 5;
    pending.action_serial = 2;
    pending.busy = Some("stopping");
    update_controls(mtm, pending);
    assert!(!row(REFRESH).isEnabled());
    assert!(row(STATUS_ROW).title().to_string().contains("正在关闭代理"));
    let mut stopped = changed;
    stopped.serial = 6;
    stopped.action_serial = 3;
    stopped.status = "已关闭";
    stopped.tun = false;
    stopped.can_start = true;
    stopped.can_stop = false;
    update_controls(mtm, stopped.clone());
    assert!(row(START).isEnabled() && !row(STOP).isEnabled());
    click(&menu, &row(START));
    assert_eq!(take(1), [Action::Start]);
    stopped.serial = 7;
    stopped.action_serial = 4;
    update_controls(mtm, stopped);
    click(&menu, &row(REFRESH));
    click(&route, &route.itemWithTag(ROUTE_SETTINGS).unwrap());
    click(&menu, &row(OPEN));
    click(&menu, &row(QUIT));
    assert_eq!(
        take(4),
        [
            Action::Refresh,
            Action::OpenRoute,
            Action::OpenWindow,
            Action::Quit
        ]
    );
    uninstall(mtm);
    assert!(!is_visible());
    assert!(with_item(mtm, |_, _, _| ()).is_none());
    uninstall(mtm);
    println!("native NSMenu: native ownership, rows, shared readback, stale intents, queued actions, AppKit delegation, and exact dispatch passed");
}

/// Disposable real WindowServer host; no product AppStorage or proxy runs.
#[cfg(target_os = "macos")]
fn real_space_host(
    app: &objc2_app_kit::NSApplication,
    mtm: objc2::MainThreadMarker,
    folder: &std::path::Path,
) {
    use macos_menu_panel::Controls;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSAccessibility, NSEventMask, NSScreen, NSWorkspace};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRect, NSString};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    app.finishLaunching();
    let actions = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&actions);
    let event_path = folder.join("events.json");
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
        let mut events = observed.lock().unwrap();
        events.push(format!("{action:?}"));
        let tmp = event_path.with_extension("tmp");
        std::fs::write(&tmp, json!(*events).to_string()).unwrap();
        std::fs::rename(tmp, &event_path).unwrap();
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
    macos_menu_panel::update_controls(
        mtm,
        Controls {
            serial: 1,
            status: "已关闭",
            can_switch: true,
            can_start: true,
            tun_supported: true,
            ..Controls::default()
        },
    );

    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 150 && !folder.join("stop").exists() {
        autoreleasepool(|_| {
            let until = NSDate::dateWithTimeIntervalSinceNow(0.035);
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
            let writes = actions
                .lock()
                .unwrap()
                .iter()
                .filter(|a| a.as_str() != "Observe")
                .count();
            if writes != 0 {
                macos_menu_panel::update_controls(
                    mtm,
                    Controls {
                        serial: writes as u64 + 1,
                        action_serial: writes as u64,
                        status: "系统代理运行中",
                        can_switch: true,
                        can_stop: true,
                        system_proxy: true,
                        tun_supported: true,
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
            let anchor = button
                .window()
                .map(|w| w.convertRectToScreen(button.convertRect_toView(button.bounds(), None)))
                .map(to_cg);
            let data = json!({
                "anchor": anchor,
                "pid": std::process::id(),
                "front": NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier()),
                "screens": screens.count(),
                "visible": macos_menu_panel::is_visible(),
            });
            let temp = folder.join("host.tmp");
            std::fs::write(&temp, data.to_string()).unwrap();
            std::fs::rename(temp, folder.join("host.json")).unwrap();
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
