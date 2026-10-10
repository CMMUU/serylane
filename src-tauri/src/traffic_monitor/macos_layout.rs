//! Shared logical-point layout and alpha-only S artwork for every macOS path.
//! Trimming the tray canvas never changes the application/Dock icon or its ink.

use std::sync::OnceLock;
use tauri::image::Image;

pub(super) const FONT_SIZE: f64 = 9.5;
pub(super) const SMALL_FONT_SIZE: f64 = 8.0;
pub(super) const LINE_HEIGHT: f64 = 10.5;
pub(super) const NUMBER_RIGHT: f64 = 32.0;
pub(super) const UNIT_LEFT: f64 = 34.0;
pub(super) const TITLE_RIGHT: f64 = 42.0;
pub(super) const IMAGE_TITLE_GAP: f64 = 2.0;
pub(super) const CONTENT_HEIGHT: f64 = 22.0;
const SIDE_INSET: f64 = 4.0;

pub(super) fn source_icon() -> &'static Image<'static> {
    static SOURCE: OnceLock<Image<'static>> = OnceLock::new();
    SOURCE.get_or_init(|| tauri::include_image!("icons/128x128.png"))
}

pub(super) fn brand_icon() -> &'static Image<'static> {
    static ICON: OnceLock<Image<'static>> = OnceLock::new();
    ICON.get_or_init(|| {
        // Keep exactly the former 16pt artwork inside an 18pt-high 2x image.
        // Only empty horizontal columns are removed, with >=1pt optical inset.
        let source = source_icon();
        let mut square = vec![0_u8; 36 * 36 * 4];
        for y in 0..32 {
            for x in 0..32 {
                let mut alpha = 0_u32;
                for sy in y * 4..(y + 1) * 4 {
                    for sx in x * 4..(x + 1) * 4 {
                        alpha += source.rgba()[((sy * 128 + sx) * 4 + 3) as usize] as u32;
                    }
                }
                let i = (((y + 2) * 36 + x + 2) * 4) as usize;
                if alpha / 16 > 0 {
                    square[i..i + 4].copy_from_slice(&[255, 255, 255, (alpha / 16) as u8]);
                }
            }
        }
        trim_horizontal(square, 36, 36)
    })
}

fn trim_horizontal(rgba: Vec<u8>, width: u32, height: u32) -> Image<'static> {
    let visible = |x| (0..height).any(|y| rgba[((y * width + x) * 4 + 3) as usize] > 0);
    let Some(left) = (0..width).find(|&x| visible(x)) else {
        return Image::new_owned(rgba, width, height);
    };
    let right = (0..width).rfind(|&x| visible(x)).unwrap();
    let cropped_width = (right - left + 1 + 4).next_multiple_of(4);
    let inset = (cropped_width - (right - left + 1)) / 2;
    let mut cropped = vec![0_u8; (cropped_width * height * 4) as usize];
    for y in 0..height {
        for x in left..=right {
            let from = ((y * width + x) * 4) as usize;
            let to = ((y * cropped_width + x - left + inset) * 4) as usize;
            cropped[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    Image::new_owned(cropped, cropped_width, height)
}

pub(super) fn title_x() -> f64 {
    brand_icon().width() as f64 / 2.0 + IMAGE_TITLE_GAP
}

pub(super) fn content_width() -> f64 {
    title_x() + TITLE_RIGHT
}

pub(super) fn item_width() -> f64 {
    content_width() + SIDE_INSET * 2.0
}

pub(super) fn bitmap_canvas() -> (Vec<u8>, u32, u32) {
    let width = (content_width() * 2.0) as u32;
    let height = (CONTENT_HEIGHT * 2.0) as u32;
    let mut rgba = vec![0_u8; (width * height * 4) as usize];
    let icon = brand_icon();
    let top = (height - icon.height()) / 2;
    for y in 0..icon.height() {
        let from = (y * icon.width() * 4) as usize;
        let to = ((y + top) * width * 4) as usize;
        rgba[to..to + (icon.width() * 4) as usize]
            .copy_from_slice(&icon.rgba()[from..from + (icon.width() * 4) as usize]);
    }
    (rgba, width, height)
}

#[cfg(test)]
mod tests {
    #[test]
    fn trimmed_mark_keeps_every_original_alpha_sample_and_vertical_size() {
        let source = super::source_icon();
        assert_eq!((source.width(), source.height()), (128, 128));
        let icon = super::brand_icon();
        assert_eq!((icon.width(), icon.height()), (24, 36));
        for y in 0..36 {
            let expected: Vec<_> = (0..32)
                .map(|x| {
                    if !(2..34).contains(&y) {
                        return 0;
                    }
                    let sy = (y - 2) * 4;
                    let sum: u32 = (sy..sy + 4)
                        .flat_map(|row| {
                            (x * 4..x * 4 + 4).map(move |col| {
                                source.rgba()[((row * 128 + col) * 4 + 3) as usize] as u32
                            })
                        })
                        .sum();
                    (sum / 16) as u8
                })
                .filter(|a| *a > 0)
                .collect();
            let actual: Vec<_> = (0..icon.width())
                .map(|x| icon.rgba()[((y * icon.width() + x) * 4 + 3) as usize])
                .filter(|a| *a > 0)
                .collect();
            assert_eq!(actual, expected, "original artwork row {y}");
        }
        assert_eq!(super::item_width(), 64.0);
        assert_eq!(super::content_width(), 56.0);
    }

    #[test]
    fn transparent_artwork_and_edge_ink_are_bounded() {
        let empty = super::trim_horizontal(vec![0; 4 * 4 * 4], 4, 4);
        assert_eq!(empty.width(), 4);
        let full = super::trim_horizontal(vec![255; 4 * 4 * 4], 4, 4);
        assert_eq!(full.width(), 8);
        assert_eq!(full.rgba()[3], 0);
        assert_eq!(full.rgba()[2 * 4 + 3], 255);
    }
}
