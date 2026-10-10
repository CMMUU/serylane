// AppKit must run on the process main thread, not a Rust test worker. This
// harness tests the production title builder against a disposable status item;
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
