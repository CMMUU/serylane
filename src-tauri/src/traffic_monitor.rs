use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use sysinfo::Networks;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "macos")]
mod macos_layout;
#[cfg(target_os = "macos")]
mod macos_title;

#[cfg(target_os = "macos")]
use std::sync::OnceLock;

pub const TRAFFIC_EVENT: &str = "global-traffic";
pub const TRAY_ID: &str = "main";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalTrafficSnapshot {
    pub enabled: bool,
    pub upload_bytes_per_second: u64,
    pub download_bytes_per_second: u64,
    pub sampled_at: Option<DateTime<Utc>>,
    pub interfaces: Vec<String>,
}

pub struct GlobalTrafficMonitor {
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    started: AtomicBool,
    snapshot: Arc<Mutex<GlobalTrafficSnapshot>>,
}

impl Default for GlobalTrafficMonitor {
    fn default() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            shutdown: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
            snapshot: Arc::new(Mutex::new(GlobalTrafficSnapshot {
                enabled: true,
                ..Default::default()
            })),
        }
    }
}

impl GlobalTrafficMonitor {
    pub fn start(&self, app: AppHandle, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.shutdown.store(false, Ordering::Release);
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let enabled_flag = Arc::clone(&self.enabled);
        let shutdown = Arc::clone(&self.shutdown);
        let snapshot = Arc::clone(&self.snapshot);
        tauri::async_runtime::spawn(async move {
            run_monitor(app, enabled_flag, shutdown, snapshot).await;
        });
    }

    pub fn set_enabled(&self, app: &AppHandle, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        let next = GlobalTrafficSnapshot {
            enabled,
            sampled_at: Some(Utc::now()),
            ..Default::default()
        };
        store_and_publish(app, &self.snapshot, next.clone());
        update_tray(app, &next);
    }

    pub fn snapshot(&self) -> GlobalTrafficSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

async fn run_monitor(
    app: AppHandle,
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    snapshot: Arc<Mutex<GlobalTrafficSnapshot>>,
) {
    let mut networks = Networks::new_with_refreshed_list();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut collecting = false;
    let mut last_sample_at = Instant::now();
    let mut last_tray_key = String::new();

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        if !enabled.load(Ordering::Acquire) {
            collecting = false;
            continue;
        }
        if !collecting {
            networks.refresh(true);
            collecting = true;
            last_sample_at = Instant::now();
            let initial = GlobalTrafficSnapshot {
                enabled: true,
                sampled_at: Some(Utc::now()),
                ..Default::default()
            };
            store_and_publish(&app, &snapshot, initial.clone());
            update_tray_if_changed(&app, &initial, &mut last_tray_key);
            continue;
        }

        networks.refresh(true);
        let elapsed = last_sample_at.elapsed().as_secs_f64().max(0.001);
        last_sample_at = Instant::now();
        let (uploaded, downloaded, interfaces) = aggregate_network_deltas(&networks);
        if !enabled.load(Ordering::Acquire) {
            collecting = false;
            continue;
        }
        let next = GlobalTrafficSnapshot {
            enabled: true,
            upload_bytes_per_second: rate_per_second(uploaded, elapsed),
            download_bytes_per_second: rate_per_second(downloaded, elapsed),
            sampled_at: Some(Utc::now()),
            interfaces,
        };
        store_and_publish(&app, &snapshot, next.clone());
        update_tray_if_changed(&app, &next, &mut last_tray_key);
    }
}

fn aggregate_network_deltas(networks: &Networks) -> (u64, u64, Vec<String>) {
    let mut uploaded = 0_u64;
    let mut downloaded = 0_u64;
    let mut interfaces = Vec::new();
    for (name, network) in networks.iter() {
        if !should_include_interface(name) {
            continue;
        }
        uploaded = uploaded.saturating_add(network.transmitted());
        downloaded = downloaded.saturating_add(network.received());
        if network.total_transmitted() > 0 || network.total_received() > 0 {
            interfaces.push(name.clone());
        }
    }
    interfaces.sort();
    (uploaded, downloaded, interfaces)
}

fn rate_per_second(bytes: u64, elapsed_seconds: f64) -> u64 {
    ((bytes as f64) / elapsed_seconds)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

fn should_include_interface(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() || matches!(name.as_str(), "lo" | "lo0" | "loopback") {
        return false;
    }
    let excluded_prefixes = [
        "utun",
        "tun",
        "tap",
        "tailscale",
        "wg",
        "wireguard",
        "docker",
        "veth",
        "virbr",
        "bridge",
        "br-",
        "awdl",
        "llw",
        "anpi",
        "gif",
        "stf",
        "vmnet",
        "vboxnet",
        "zt",
        "zerotier",
        "ham",
        "nordlynx",
    ];
    !excluded_prefixes
        .iter()
        .any(|prefix| name.starts_with(prefix))
        && !name.contains("loopback")
        && !name.contains("virtual")
        && !name.contains(" vpn")
}

fn store_and_publish(
    app: &AppHandle,
    state: &Arc<Mutex<GlobalTrafficSnapshot>>,
    next: GlobalTrafficSnapshot,
) {
    if let Ok(mut snapshot) = state.lock() {
        *snapshot = next.clone();
    }
    let _ = app.emit(TRAFFIC_EVENT, next);
}

fn update_tray_if_changed(
    app: &AppHandle,
    snapshot: &GlobalTrafficSnapshot,
    last_key: &mut String,
) {
    #[cfg(target_os = "macos")]
    let key = format!(
        "{}:{}",
        snapshot.enabled,
        macos_title::display_key(
            snapshot.upload_bytes_per_second,
            snapshot.download_bytes_per_second
        )
    );
    #[cfg(not(target_os = "macos"))]
    let key = format!(
        "{}:{}:{}",
        snapshot.enabled,
        format_tray_rate(snapshot.upload_bytes_per_second),
        format_tray_rate(snapshot.download_bytes_per_second)
    );
    if key == *last_key {
        return;
    }
    *last_key = key;
    update_tray(app, snapshot);
}

fn update_tray(app: &AppHandle, snapshot: &GlobalTrafficSnapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        eprintln!("global traffic tray icon not found");
        return;
    };
    if !snapshot.enabled {
        // tray-icon 0.24's macOS None setter leaves the previous native title
        // intact. An empty string explicitly removes both rows.
        let _ = tray.set_title(Some(""));
        #[cfg(target_os = "macos")]
        if let Err(error) = macos_title::clear(&tray) {
            eprintln!("global traffic native tray reset failed: {error}");
        }
        if let Some(icon) = app.default_window_icon() {
            let _ = tray.set_icon_with_as_template(Some(icon.clone()), false);
        }
        let _ = tray.set_tooltip(Some("Serylane"));
        return;
    }

    #[cfg(not(target_os = "macos"))]
    let upload = format_tray_rate(snapshot.upload_bytes_per_second);
    #[cfg(not(target_os = "macos"))]
    let download = format_tray_rate(snapshot.download_bytes_per_second);
    #[cfg(not(target_os = "macos"))]
    let tooltip = format!("Serylane\n↑ {upload}\n↓ {download}");

    #[cfg(target_os = "macos")]
    {
        // Only the shared S mark remains an image. Numbers use AppKit text at
        // its real point size, independently of tray-icon's 18pt image scaling.
        let native = tray
            .set_icon_with_as_template(Some(native_macos_brand_icon()), true)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                macos_title::update(
                    &tray,
                    snapshot.upload_bytes_per_second,
                    snapshot.download_bytes_per_second,
                )
            });
        match native {
            Ok(()) => return,
            Err(error) => eprintln!("global traffic native tray fallback: {error}"),
        }
        let _ = tray.set_title(Some(""));
        let _ = macos_title::clear(&tray);
        let (icon, is_template) = render_macos_tray_icon(
            snapshot.upload_bytes_per_second,
            snapshot.download_bytes_per_second,
        );
        if let Err(error) = tray.set_icon_with_as_template(Some(icon), is_template) {
            eprintln!("global traffic tray icon update failed: {error}");
        } else if let Err(error) = macos_title::update_bitmap(
            &tray,
            snapshot.upload_bytes_per_second,
            snapshot.download_bytes_per_second,
        ) {
            eprintln!("global traffic bitmap tray layout failed: {error}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = tray.set_title(Some(format!("↑ {upload}\n↓ {download}")));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = tray.set_tooltip(Some(tooltip));
}

#[cfg(target_os = "macos")]
fn native_macos_brand_icon() -> tauri::image::Image<'static> {
    macos_layout::brand_icon().clone()
}

#[cfg(any(not(target_os = "macos"), test))]
fn format_tray_rate(bytes_per_second: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes_per_second as f64;
    let (value, unit) = if bytes >= GIB {
        (bytes / GIB, "G/s")
    } else if bytes >= MIB {
        (bytes / MIB, "M/s")
    } else if bytes >= KIB {
        (bytes / KIB, "K/s")
    } else {
        (bytes, "B/s")
    };
    if value < 10.0 && unit != "B/s" {
        format!("{value:.1}{unit}")
    } else {
        format!("{:.0}{unit}", value.min(999.0))
    }
}

#[cfg(target_os = "macos")]
fn render_macos_tray_icon(upload: u64, download: u64) -> (tauri::image::Image<'static>, bool) {
    if let Some(font) = macos_status_font() {
        return (
            render_monochrome_macos_tray_icon(font, upload, download),
            true,
        );
    }
    (render_pixel_macos_tray_icon(upload, download), true)
}

#[cfg(target_os = "macos")]
static MACOS_STATUS_FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn macos_status_font() -> Option<&'static fontdue::Font> {
    MACOS_STATUS_FONT
        .get_or_init(|| {
            [
                "/System/Library/Fonts/SFNS.ttf",
                "/System/Library/Fonts/SFNSMono.ttf",
                "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            ]
            .iter()
            .find_map(|path| {
                std::fs::read(path).ok().and_then(|bytes| {
                    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
                })
            })
        })
        .as_ref()
}

#[cfg(target_os = "macos")]
fn render_monochrome_macos_tray_icon(
    font: &fontdue::Font,
    upload: u64,
    download: u64,
) -> tauri::image::Image<'static> {
    let (mut rgba, width, height) = macos_layout::bitmap_canvas();
    let text_x = (macos_layout::title_x() * 2.0) as u32;
    let color = [255, 255, 255, 255];
    let size = (macos_layout::FONT_SIZE * 2.0) as f32;
    for (bytes, row, up) in [(upload, 0, true), (download, 21, false)] {
        let rate = macos_title::format_rate(bytes);
        draw_smooth_arrow(
            &mut rgba,
            width,
            height,
            text_x as f32 + 4.0,
            row as f32 + 3.0,
            up,
            color,
        );
        let right = text_x as f32 + (macos_layout::NUMBER_RIGHT * 2.0) as f32;
        let x = (right - smooth_text_width(font, &rate.number, size)).round() as u32;
        draw_smooth_text(
            &mut rgba,
            width,
            height,
            font,
            &rate.number,
            x,
            row + 18,
            size,
            color,
        );
        draw_smooth_text(
            &mut rgba,
            width,
            height,
            font,
            rate.unit,
            text_x + (macos_layout::UNIT_LEFT * 2.0) as u32,
            row + 18,
            (macos_layout::SMALL_FONT_SIZE * 2.0) as f32,
            color,
        );
    }
    tauri::image::Image::new_owned(rgba, width, height)
}

#[cfg(target_os = "macos")]
fn smooth_text_width(font: &fontdue::Font, text: &str, size: f32) -> f32 {
    let mut width = 0.0_f32;
    let mut previous = None;
    for character in text.chars() {
        if let Some(previous) = previous {
            width += font
                .horizontal_kern(previous, character, size)
                .unwrap_or(0.0);
        }
        width += font.metrics(character, size).advance_width;
        previous = Some(character);
    }
    width
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_smooth_text(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    font: &fontdue::Font,
    text: &str,
    x: u32,
    baseline: i32,
    size: f32,
    color: [u8; 4],
) {
    let mut cursor = x as f32;
    let mut previous = None;
    for character in text.chars() {
        if let Some(previous) = previous {
            cursor += font
                .horizontal_kern(previous, character, size)
                .unwrap_or(0.0);
        }
        let (metrics, bitmap) = font.rasterize(character, size);
        let glyph_x = cursor.round() as i32 + metrics.xmin;
        let glyph_y = baseline - metrics.ymin - metrics.height as i32;
        for row in 0..metrics.height {
            for column in 0..metrics.width {
                let coverage = bitmap[row * metrics.width + column];
                if coverage == 0 {
                    continue;
                }
                blend_pixel(
                    rgba,
                    width,
                    height,
                    glyph_x + column as i32,
                    glyph_y + row as i32,
                    color,
                    coverage,
                );
                blend_pixel(
                    rgba,
                    width,
                    height,
                    glyph_x + column as i32 + 1,
                    glyph_y + row as i32,
                    color,
                    coverage,
                );
            }
        }
        cursor += metrics.advance_width;
        previous = Some(character);
    }
}

#[cfg(target_os = "macos")]
fn draw_smooth_arrow(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    center_x: f32,
    y: f32,
    up: bool,
    color: [u8; 4],
) {
    let tip = if up { y } else { y + 12.0 };
    let shoulder = if up { tip + 4.0 } else { tip - 4.0 };
    draw_line_segment(
        rgba,
        width,
        height,
        center_x,
        y,
        center_x,
        y + 12.0,
        1.5,
        color,
    );
    for dx in [-4.0, 4.0] {
        draw_line_segment(
            rgba,
            width,
            height,
            center_x,
            tip,
            center_x + dx,
            shoulder,
            1.5,
            color,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_line_segment(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let min_x = (x1.min(x2) - thickness).floor().max(0.0) as i32;
    let max_x = (x1.max(x2) + thickness).ceil().min(width as f32 - 1.0) as i32;
    let min_y = (y1.min(y2) - thickness).floor().max(0.0) as i32;
    let max_y = (y1.max(y2) + thickness).ceil().min(height as f32 - 1.0) as i32;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let length_squared = (dx * dx + dy * dy).max(f32::EPSILON);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = (((px - x1) * dx + (py - y1) * dy) / length_squared).clamp(0.0, 1.0);
            let nearest_x = x1 + t * dx;
            let nearest_y = y1 + t * dy;
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
            let coverage = ((thickness * 0.5 + 0.75 - distance) * 255.0).clamp(0.0, 255.0) as u8;
            if coverage > 0 {
                blend_pixel(rgba, width, height, x, y, color, coverage);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn blend_pixel(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: [u8; 4],
    coverage: u8,
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let index = ((y as u32 * width + x as u32) * 4) as usize;
    let alpha = ((coverage as u16 * color[3] as u16) / 255) as u8;
    if alpha >= rgba[index + 3] {
        rgba[index] = color[0];
        rgba[index + 1] = color[1];
        rgba[index + 2] = color[2];
        rgba[index + 3] = alpha;
    }
}

#[cfg(target_os = "macos")]
fn render_pixel_macos_tray_icon(upload: u64, download: u64) -> tauri::image::Image<'static> {
    let (mut rgba, width, height) = macos_layout::bitmap_canvas();
    let text_x = (macos_layout::title_x() * 2.0) as u32;
    for (bytes, row, up) in [(upload, 0, true), (download, 21, false)] {
        let rate = macos_title::format_rate(bytes);
        draw_arrow(&mut rgba, width, height, text_x, row + 2, up);
        let right = text_x + (macos_layout::NUMBER_RIGHT * 2.0) as u32;
        draw_text(
            &mut rgba,
            width,
            height,
            right - pixel_text_width(&rate.number, 2),
            row + 4,
            &rate.number,
            2,
            255,
        );
        draw_text(
            &mut rgba,
            width,
            height,
            text_x + (macos_layout::UNIT_LEFT * 2.0) as u32,
            row + 4,
            rate.unit,
            2,
            255,
        );
    }
    tauri::image::Image::new_owned(rgba, width, height)
}

#[cfg(target_os = "macos")]
fn pixel_text_width(text: &str, scale: u32) -> u32 {
    let glyphs = text.chars().count() as u32;
    glyphs.saturating_mul(6 * scale).saturating_sub(scale)
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_text(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
    scale: u32,
    alpha: u8,
) {
    let mut cursor = x;
    for character in text.chars() {
        if let Some(rows) = glyph(character) {
            for (row, bits) in rows.into_iter().enumerate() {
                for column in 0..5_u32 {
                    if bits & (1 << (4 - column)) == 0 {
                        continue;
                    }
                    draw_rect(
                        rgba,
                        width,
                        height,
                        cursor + column * scale,
                        y + row as u32 * scale,
                        scale,
                        scale,
                        alpha,
                    );
                }
            }
        }
        cursor = cursor.saturating_add(6 * scale);
    }
}

#[cfg(target_os = "macos")]
fn draw_arrow(rgba: &mut [u8], width: u32, height: u32, x: u32, y: u32, up: bool) {
    let center = x + 4;
    if up {
        draw_rect(rgba, width, height, center, y + 3, 2, 11, 255);
        for offset in 0..4_u32 {
            draw_rect(
                rgba,
                width,
                height,
                center - offset,
                y + 3 + offset,
                2,
                2,
                255,
            );
            draw_rect(
                rgba,
                width,
                height,
                center + offset,
                y + 3 + offset,
                2,
                2,
                255,
            );
        }
    } else {
        draw_rect(rgba, width, height, center, y, 2, 11, 255);
        for offset in 0..4_u32 {
            draw_rect(
                rgba,
                width,
                height,
                center - offset,
                y + 9 - offset,
                2,
                2,
                255,
            );
            draw_rect(
                rgba,
                width,
                height,
                center + offset,
                y + 9 - offset,
                2,
                2,
                255,
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn draw_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    rect_width: u32,
    rect_height: u32,
    alpha: u8,
) {
    for py in y..y.saturating_add(rect_height).min(height) {
        for px in x..x.saturating_add(rect_width).min(width) {
            let index = ((py * width + px) * 4) as usize;
            rgba[index] = 255;
            rgba[index + 1] = 255;
            rgba[index + 2] = 255;
            rgba[index + 3] = alpha;
        }
    }
}

#[cfg(target_os = "macos")]
fn glyph(character: char) -> Option<[u8; 7]> {
    Some(match character {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        's' => [0, 0, 15, 16, 14, 1, 30],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{format_tray_rate, rate_per_second, should_include_interface};

    #[cfg(target_os = "macos")]
    #[test]
    fn native_and_fallback_trays_use_the_shared_s_mark() {
        let native = super::native_macos_brand_icon();
        let fallback = super::render_pixel_macos_tray_icon(1_258_291, 8_500);
        let font = super::macos_status_font().expect("macOS system status font");
        let normal = super::render_monochrome_macos_tray_icon(font, 1_258_291, 8_500);
        for image in [&normal, &fallback] {
            assert_eq!((image.width(), image.height()), (112, 44));
            let top = (image.height() - native.height()) / 2;
            for y in 0..native.height() {
                for x in 0..native.width() {
                    let alpha = (((y + top) * image.width() + x) * 4 + 3) as usize;
                    assert_eq!(
                        image.rgba()[alpha],
                        native.rgba()[((y * native.width() + x) * 4 + 3) as usize]
                    );
                }
            }
        }
        // Optional local visual evidence, never used by production or required
        // in CI. The bytes contain only the public icon and synthetic rates.
        if let Some(folder) = std::env::var_os("SERYLANE_TRAY_SNAPSHOT_DIR") {
            let folder = std::path::PathBuf::from(folder);
            std::fs::create_dir_all(&folder).unwrap();
            for (name, image) in [
                ("native-mark", native),
                ("normal", normal),
                ("fallback", fallback),
            ] {
                std::fs::write(folder.join(format!("{name}.rgba")), image.rgba()).unwrap();
                std::fs::write(
                    folder.join(format!("{name}.json")),
                    format!(
                        "{{\"width\":{},\"height\":{}}}",
                        image.width(),
                        image.height()
                    ),
                )
                .unwrap();
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_bitmap_rate_has_stable_width_two_rows_and_clear_edges() {
        let font = super::macos_status_font().expect("macOS system status font");
        for rate in [
            0,
            1,
            1023,
            1024,
            10_188,
            10_189,
            999 * 1024,
            1023 * 1024,
            1024 * 1024 - 1,
            1 << 40,
            1 << 50,
            u64::MAX,
        ] {
            let label = super::macos_title::format_rate(rate);
            assert!(label.unit.chars().all(|c| super::glyph(c).is_some()));
            assert!(super::smooth_text_width(font, &label.number, 19.0) <= 48.0);
            assert!(super::smooth_text_width(font, label.unit, 16.0) <= 16.0);
            for image in [
                super::render_monochrome_macos_tray_icon(font, rate, rate),
                super::render_pixel_macos_tray_icon(rate, rate),
            ] {
                assert_eq!((image.width(), image.height()), (112, 44));
                let ink =
                    |x: u32, y: u32| image.rgba()[((y * image.width() + x) * 4 + 3) as usize] > 0;
                assert!(
                    (0..image.width()).all(|x| !ink(x, 0) && !ink(x, image.height() - 1)),
                    "vertical clipping at {rate}"
                );
                assert!(
                    (0..image.height()).all(|y| !ink(0, y) && !ink(image.width() - 1, y)),
                    "horizontal clipping at {rate}"
                );
                let text_x = (super::macos_layout::title_x() * 2.0) as u32;
                let rows: Vec<_> = (0..image.height())
                    .map(|y| (text_x..image.width()).any(|x| ink(x, y)))
                    .collect();
                let runs = rows
                    .iter()
                    .enumerate()
                    .filter(|(y, v)| **v && (*y == 0 || !rows[y - 1]))
                    .count();
                assert_eq!(runs, 2, "both bitmap rows must remain separate at {rate}");
            }
        }
    }

    #[test]
    fn filters_virtual_interfaces_without_excluding_physical_names() {
        for name in ["en0", "en7", "eth0", "wlan0", "Ethernet", "Wi-Fi"] {
            assert!(should_include_interface(name), "{name}");
        }
        for name in [
            "lo0",
            "utun8",
            "tailscale0",
            "docker0",
            "veth1234",
            "bridge0",
            "awdl0",
            "VMware Virtual Ethernet Adapter",
        ] {
            assert!(!should_include_interface(name), "{name}");
        }
    }

    #[test]
    fn formats_compact_tray_rates() {
        assert_eq!(format_tray_rate(0), "0B/s");
        assert_eq!(format_tray_rate(1_536), "1.5K/s");
        assert_eq!(format_tray_rate(1_258_291), "1.2M/s");
        assert_eq!(format_tray_rate(12 * 1024 * 1024), "12M/s");
    }

    #[test]
    fn normalizes_delta_by_elapsed_time() {
        assert_eq!(rate_per_second(2_000, 2.0), 1_000);
    }
}
