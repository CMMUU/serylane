//! Native, two-row macOS traffic title. No custom status-item view or bitmap
//! text: the existing Tauri button keeps ownership of its menu and events.

use objc2::{rc::Retained, AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBaselineOffsetAttributeName, NSCellImagePosition, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSLineBreakMode, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSStatusItem, NSTextAlignment, NSTextTab,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSNumber, NSRange,
    NSSize, NSString,
};

use super::macos_layout::{
    self, FONT_SIZE, LINE_HEIGHT, NUMBER_RIGHT, SMALL_FONT_SIZE, TITLE_RIGHT, UNIT_LEFT,
};
const OPTICAL_BASELINE_INSET: f64 = 1.5;
const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];
const FULL_UNITS: [&str; 7] = ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s", "PiB/s", "EiB/s"];

#[derive(Debug, PartialEq)]
pub(super) struct Rate {
    pub number: String,
    pub unit: &'static str,
    pub full_unit: &'static str,
}

pub(super) fn format_rate(bytes: u64) -> Rate {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    // Carry rounded boundary values rather than displaying 1024K or silently
    // clamping large rates to 999G. Four numeric glyphs always fit the column.
    if value.round() >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let number = if unit > 0 && value < 9.95 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    Rate {
        number,
        unit: UNITS[unit],
        full_unit: FULL_UNITS[unit],
    }
}

pub(super) fn display_key(upload: u64, download: u64) -> String {
    let up = format_rate(upload);
    let down = format_rate(download);
    format!("{}{}:{}{}", up.number, up.unit, down.number, down.unit)
}

pub(super) fn tooltip(upload: u64, download: u64) -> String {
    let up = format_rate(upload);
    let down = format_rate(download);
    format!(
        "Serylane\n↑ {} {}\n↓ {} {}",
        up.number, up.full_unit, down.number, down.full_unit
    )
}

pub(super) fn build_title(upload: u64, download: u64) -> Retained<NSMutableAttributedString> {
    let up = format_rate(upload);
    let down = format_rate(download);
    // The final zero-width marker anchors the intrinsic title width too.
    // A fixed NSStatusItem length alone still lets NSButton center K and M
    // titles differently, shifting every column by a fraction of a point.
    let top = format!("↑\t{}\t{}\t\u{200b}", up.number, up.unit);
    let bottom = format!("↓\t{}\t{}\t\u{200b}", down.number, down.unit);
    let title = NSMutableAttributedString::initWithString(
        NSMutableAttributedString::alloc(),
        &NSString::from_str(&format!("{top}\n{bottom}")),
    );
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Left);
    paragraph.setMinimumLineHeight(LINE_HEIGHT);
    paragraph.setMaximumLineHeight(LINE_HEIGHT);
    paragraph.setLineBreakMode(NSLineBreakMode::ByClipping);
    // Explicit tabs, not padding spaces: changing digits or units cannot move
    // the arrow, numeric right edge or unit column.
    unsafe {
        let tabs = NSArray::from_retained_slice(&[
            NSTextTab::initWithTextAlignment_location_options(
                NSTextTab::alloc(),
                NSTextAlignment::Right,
                NUMBER_RIGHT,
                &NSDictionary::new(),
            ),
            NSTextTab::initWithTextAlignment_location_options(
                NSTextTab::alloc(),
                NSTextAlignment::Left,
                UNIT_LEFT,
                &NSDictionary::new(),
            ),
            NSTextTab::initWithTextAlignment_location_options(
                NSTextTab::alloc(),
                NSTextAlignment::Left,
                TITLE_RIGHT,
                &NSDictionary::new(),
            ),
        ]);
        paragraph.setTabStops(Some(&tabs));
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, NSFontWeightMedium);
        let small = NSFont::systemFontOfSize_weight(SMALL_FONT_SIZE, NSFontWeightRegular);
        let all = NSRange::new(0, title.length());
        // Each value has the type required by Apple's corresponding attribute
        // key; every range is measured in UTF-16 code units, including arrows.
        title.addAttribute_value_range(NSFontAttributeName, &font, all);
        title.addAttribute_value_range(NSParagraphStyleAttributeName, &paragraph, all);
        let bottom_start = top.encode_utf16().count() + 1;
        for (start, number) in [(0, &up.number), (bottom_start, &down.number)] {
            title.addAttribute_value_range(NSFontAttributeName, &small, NSRange::new(start, 1));
            title.addAttribute_value_range(
                NSFontAttributeName,
                &small,
                NSRange::new(start + 3 + number.len(), 1),
            );
        }
    }
    // Leave foreground color unset. NSStatusBarButton applies the correct
    // menu-bar foreground and selection appearance to every text run.
    title
}

pub(super) fn apply_to_item(
    item: &NSStatusItem,
    upload: u64,
    download: u64,
    mtm: MainThreadMarker,
) -> Result<(), String> {
    let button = item.button(mtm).ok_or("native status button is missing")?;
    let cell = button
        .cell()
        .ok_or("native status button cell is missing")?;
    cell.setWraps(false);
    cell.setUsesSingleLineMode(false);
    cell.setLineBreakMode(NSLineBreakMode::ByClipping);
    // NSStatusBarButton also uses its control font for vertical placement;
    // setting attributed runs alone leaves the default menu font's baseline.
    let font =
        unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, NSFontWeightMedium) };
    button.setFont(Some(&font));
    button.setAlignment(NSTextAlignment::Left);
    button.setImagePosition(NSCellImagePosition::ImageLeft);
    let title = build_title(upload, download);
    button.setAttributedTitle(&title);
    item.setLength(macos_layout::item_width());
    // NSStatusBarButton's cell can anchor a multiline title at the original
    // single-line baseline (unlike an ordinary NSButton). Center the actual
    // two-line title using the live button geometry, not an OS-specific offset.
    let bounds = button.bounds();
    let rect = cell.titleRectForBounds(bounds);
    let displacement =
        rect.origin.y + rect.size.height / 2.0 - (bounds.origin.y + bounds.size.height / 2.0);
    let geometric_offset = if button.isFlipped() {
        displacement
    } else {
        -displacement
    };
    // The typographic box includes unused descent below these digits/arrows.
    // macOS 14/15's dark 2x rendering otherwise touches the top edge even when
    // the 21pt box is centered. Move ink down 1.5pt, preserving both row baselines
    // and a visible top/bottom inset on the old and current status-button cells.
    let offset = geometric_offset - OPTICAL_BASELINE_INSET;
    unsafe {
        title.addAttribute_value_range(
            NSBaselineOffsetAttributeName,
            &NSNumber::new_f64(offset),
            NSRange::new(0, title.length()),
        );
    }
    button.setAttributedTitle(&title);
    Ok(())
}

pub(super) fn apply_bitmap_to_item(
    item: &NSStatusItem,
    mtm: MainThreadMarker,
) -> Result<(), String> {
    let button = item.button(mtm).ok_or("native status button is missing")?;
    let image = button.image().ok_or("bitmap status image is missing")?;
    // tray-icon sizes all images to 18pt. Two rows need their real 22pt height,
    // not a shrunken font; refresh its hit area after setting the same 64pt slot.
    image.setSize(NSSize::new(
        macos_layout::content_width(),
        macos_layout::CONTENT_HEIGHT,
    ));
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    item.setLength(macos_layout::item_width());
    Ok(())
}

pub(super) fn update_bitmap(
    tray: &tauri::tray::TrayIcon,
    upload: u64,
    download: u64,
) -> Result<(), String> {
    tray.with_inner_tray_icon(move |inner| {
        let mtm = MainThreadMarker::new().ok_or("bitmap tray update is not on the main thread")?;
        let item = inner
            .ns_status_item()
            .ok_or("native status item is missing")?;
        apply_bitmap_to_item(&item, mtm)?;
        inner
            .set_tooltip(Some(tooltip(upload, download)))
            .map_err(|error| error.to_string())
    })
    .map_err(|error| error.to_string())?
}

pub(super) fn clear_item(item: &NSStatusItem, mtm: MainThreadMarker) -> Result<(), String> {
    let button = item.button(mtm).ok_or("native status button is missing")?;
    button.setAttributedTitle(&NSAttributedString::initWithString(
        NSAttributedString::alloc(),
        &NSString::from_str(""),
    ));
    button.setAlignment(NSTextAlignment::Center);
    item.setLength(NSVariableStatusItemLength);
    Ok(())
}

pub(super) fn update(
    tray: &tauri::tray::TrayIcon,
    upload: u64,
    download: u64,
) -> Result<(), String> {
    tray.with_inner_tray_icon(move |inner| {
        let mtm = MainThreadMarker::new().ok_or("native tray update is not on the main thread")?;
        let item = inner
            .ns_status_item()
            .ok_or("native status item is missing")?;
        apply_to_item(&item, upload, download, mtm)?;
        // This public setter also refreshes tray-icon's event/hover view after
        // the native title and fixed item width have changed.
        inner
            .set_tooltip(Some(tooltip(upload, download)))
            .map_err(|error| error.to_string())
    })
    .map_err(|error| error.to_string())?
}

pub(super) fn clear(tray: &tauri::tray::TrayIcon) -> Result<(), String> {
    tray.with_inner_tray_icon(|inner| {
        let mtm = MainThreadMarker::new().ok_or("native tray reset is not on the main thread")?;
        let item = inner
            .ns_status_item()
            .ok_or("native status item is missing")?;
        clear_item(&item, mtm)?;
        inner
            .set_tooltip(Some("Serylane"))
            .map_err(|error| error.to_string())
    })
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn compact_rates_fit_fixed_columns_and_carry_unit_boundaries() {
        use super::format_rate;
        for (bytes, number, unit) in [
            (0, "0", "B"),
            (1023, "1023", "B"),
            (1024, "1.0", "K"),
            (1536, "1.5", "K"),
            (10189, "10", "K"),
            (1024 * 1024 - 1, "1.0", "M"),
            (12 * 1024 * 1024, "12", "M"),
            (1_u64 << 40, "1.0", "T"),
            (u64::MAX, "16", "E"),
        ] {
            let rate = format_rate(bytes);
            assert_eq!((rate.number.as_str(), rate.unit), (number, unit), "{bytes}");
            assert!(rate.number.len() <= 4);
        }
    }

    #[test]
    fn tooltip_keeps_explicit_binary_units_and_dedupe_tracks_large_rates() {
        use super::{display_key, tooltip};
        assert_eq!(
            tooltip(1536, 1024 * 1024),
            "Serylane\n↑ 1.5 KiB/s\n↓ 1.0 MiB/s"
        );
        assert_ne!(display_key(1 << 40, 0), display_key(2 << 40, 0));
    }
}
