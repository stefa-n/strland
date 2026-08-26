use crate::config::Config;
use crate::win;
use eframe::egui;

/// Bundled control-center icon textures.
pub struct ControlCenterIcons {
    pub wifi: egui::TextureHandle,
    pub audio: egui::TextureHandle,
    pub audio_muted: egui::TextureHandle,
    pub bt: egui::TextureHandle,
    pub moon: egui::TextureHandle,
    pub night: egui::TextureHandle,
    pub power: egui::TextureHandle,
    pub wallpaper: egui::TextureHandle,
    pub theme: egui::TextureHandle,
}

/// Which submenu the control center is currently showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ControlSubmenu {
    None,
    Audio,
    Wifi,
    Bluetooth,
    Power,
}

/// User action fired by the control center.
pub enum ControlAction {
    OpenSubmenu(ControlSubmenu),
    CloseSubmenu,
    ToggleMute,
    ToggleWifi,
    ToggleBluetooth,
    ToggleDnd,
    ToggleNightLight,
    SetDefaultAudio(String),
    ConnectWifi(String),
    OpenWallpapers,
    OpenThemes,
}

/// Mutable state for the control-center toggles (persisted across frames).
#[derive(Clone)]
pub struct ControlCenterState {
    pub dnd: bool,
    pub night_light: bool,
    pub submenu: ControlSubmenu,
    pub cached_wifi_on: bool,
    pub cached_wifi_name: String,
    pub cached_bt_on: bool,
    pub cached_bt_name: String,
    pub cached_audio_name: String,
    pub cached_muted: bool,
    pub cached_audio_devices: Vec<(String, String)>,
    pub cached_wifi_networks: Vec<String>,
    pub cached_bt_devices: Vec<(String, bool)>,
    pub last_poll: std::time::Instant,
}

impl Default for ControlCenterState {
    fn default() -> Self {
        Self {
            dnd: false,
            night_light: false,
            submenu: ControlSubmenu::None,
            cached_wifi_on: false,
            cached_wifi_name: String::new(),
            cached_bt_on: false,
            cached_bt_name: String::new(),
            cached_audio_name: String::new(),
            cached_muted: false,
            cached_audio_devices: Vec::new(),
            cached_wifi_networks: Vec::new(),
            cached_bt_devices: Vec::new(),
            last_poll: std::time::Instant::now(),
        }
    }
}

/// A rounded "tile". `zone` selects whether the icon or the text was hit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileZone {
    None,
    Icon,
    Text,
}

#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &egui::Ui,
    rect: egui::Rect,
    label: &str,
    sublabel: &str,
    icon: Option<&egui::TextureHandle>,
    accent: egui::Color32,
    alpha: f32,
    on: bool,
    item_bg: egui::Color32,
    bg_color: egui::Color32,
) -> TileZone {
    let response = ui.interact(rect, ui.make_persistent_id(("cctile", label)), egui::Sense::click());
    let hovered = response.hovered() || on;

    let (bg, fg) = if on {
        (
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), 255),
            bg_color,
        )
    } else {
        let lift = if hovered { 14.0 } else { 0.0 };
        let base = item_bg;
        (
            egui::Color32::from_rgba_premultiplied(
                (base.r() as f32 + lift).min(255.0) as u8,
                (base.g() as f32 + lift).min(255.0) as u8,
                (base.b() as f32 + lift).min(255.0) as u8,
                base.a(),
            ),
            accent,
        )
    };

    ui.painter().rect_filled(rect, egui::CornerRadius::same(26), bg);

    let icon_size = 22.0;
    if let Some(icon) = icon {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::vec2(icon_size, icon_size),
        );
        ui.painter().image(
            icon.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), (alpha * 255.0) as u8),
        );
    }

    ui.painter().text(
        egui::pos2(rect.left() + 50.0, rect.center().y - 7.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        egui::Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), (alpha * 255.0) as u8),
    );
    // Truncated sublabel to prevent overflow.
    let sub_font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let max_w = rect.width() - 58.0;
    let truncated_sub = truncate_to_fit(ui.painter(), sublabel, &sub_font, max_w);
    ui.painter().text(
        egui::pos2(rect.left() + 50.0, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        &truncated_sub,
        sub_font,
        egui::Color32::from_rgba_premultiplied(fg.r(), fg.g(), fg.b(), (alpha * 255.0 * 0.7) as u8),
    );

    if response.clicked() {
        // Icon zone is the left ~40% of the tile; the rest is the text zone.
        if response.interact_pointer_pos().map(|p| p.x < rect.left() + rect.width() * 0.42).unwrap_or(false) {
            TileZone::Icon
        } else {
            TileZone::Text
        }
    } else {
        TileZone::None
    }
}

#[allow(clippy::too_many_arguments)]
fn slider_row(
    ui: &egui::Ui,
    y: f32,
    x: f32,
    width: f32,
    height: f32,
    icon: Option<&egui::TextureHandle>,
    accent: egui::Color32,
    value: f32,
    id: &'static str,
    on_change: impl FnOnce(f32),
    item_bg: egui::Color32,
) -> bool {
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
    ui.painter().rect_filled(rect, egui::CornerRadius::same((height / 2.0) as u8), item_bg);

    let icon_size = 20.0;
    let icon_x = x + 16.0;
    if let Some(icon) = icon {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(icon_x + icon_size / 2.0, rect.center().y),
            egui::vec2(icon_size, icon_size),
        );
        ui.painter().image(
            icon.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            accent,
        );
    }

    let slider_x = icon_x + icon_size + 6.0;
    let slider_w = rect.right() - slider_x - 6.0;
    let r = height / 2.0;
    let fill_w = slider_w * value.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        let fill_rect = egui::Rect::from_min_size(
            egui::pos2(slider_x, rect.center().y - r),
            egui::vec2(fill_w, height),
        );
        ui.painter().rect_filled(
            fill_rect,
            egui::CornerRadius::same(r as u8),
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), 255),
        );
    }

    let response = ui.interact(rect, ui.make_persistent_id(("cslider", id)), egui::Sense::drag());
    let dragging = response.interact_pointer_pos().is_some();
    if let Some(pos) = response.interact_pointer_pos() {
        let frac = ((pos.x - slider_x) / slider_w).clamp(0.0, 1.0);
        on_change(frac);
    }
    dragging
}

/// Draws a submenu as a column of simple list rows.
fn submenu(
    ui: &egui::Ui,
    rect: egui::Rect,
    accent: egui::Color32,
    title: &str,
    rows: Vec<(String, bool)>, // (label, is_current)
    item_bg: egui::Color32,
) -> Option<usize> {
    ui.painter().rect_filled(rect, egui::CornerRadius::same(22), item_bg);

    let pad = 14.0;
    ui.painter().text(
        egui::pos2(rect.left() + pad, rect.top() + pad),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
        accent,
    );

    let mut hit: Option<usize> = None;
    let mut y = rect.top() + pad + 24.0;
    for (i, (label, current)) in rows.iter().enumerate() {
        let row = egui::Rect::from_min_size(
            egui::pos2(rect.left() + pad, y),
            egui::vec2(rect.width() - pad * 2.0, 30.0),
        );
        let resp = ui.interact(row, ui.make_persistent_id(("ccsub", title, i)), egui::Sense::click());
        let hovered = resp.hovered();
        let bgc = if *current {
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), 200)
        } else if hovered {
            egui::Color32::from_rgb(42, 42, 44)
        } else {
            egui::Color32::TRANSPARENT
        };
        if bgc.a() > 0 {
            ui.painter().rect_filled(row, egui::CornerRadius::same(8), bgc);
        }
        ui.painter().text(
            egui::pos2(row.left() + 8.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            accent,
        );
        if resp.clicked() {
            hit = Some(i);
        }
        y += 36.0;
    }
    hit
}

/// Draws the full control-center panel inside `rect`. Returns actions for the
/// caller to apply (opening submenus, toggling radios, selecting devices).
#[allow(clippy::too_many_arguments)]
pub fn draw_control_center(
    ui: &egui::Ui,
    rect: egui::Rect,
    cfg: &Config,
    accent: egui::Color32,
    state: &mut ControlCenterState,
    icons: &ControlCenterIcons,
    media: Option<(&egui::TextureHandle, &str, &str)>,
    volume: f32,
    bg: egui::Color32,
    item_bg: egui::Color32,
) -> Vec<ControlAction> {
    let mut actions: Vec<ControlAction> = Vec::new();
    ui.painter().rect_filled(rect, egui::CornerRadius::same(30), bg);

    let pad = 16.0;
    let gap = 10.0;
    let tile_h = 62.0;
    let content_w = rect.width() - pad * 2.0;
    let tile_w = (content_w - gap) / 2.0;

    let y0 = rect.top() + pad;
    let row1 = [
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad, y0), egui::vec2(tile_w, tile_h)),
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad + tile_w + gap, y0), egui::vec2(tile_w, tile_h)),
    ];
    let y1 = y0 + tile_h + gap;
    let row2 = [
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad, y1), egui::vec2(tile_w, tile_h)),
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad + tile_w + gap, y1), egui::vec2(tile_w, tile_h)),
    ];

    let status = win::system_status();

    // Poll expensive COM/WinRT queries at most every 500ms.
    let now = std::time::Instant::now();
    if now.duration_since(state.last_poll).as_millis() >= 500 {
        state.cached_wifi_on = win::wifi_radio_is_on();
        state.cached_wifi_name = win::current_wifi_ssid();
        state.cached_bt_on = win::bluetooth_radio_is_on();
        state.cached_bt_name = win::current_bluetooth_name();
        state.cached_audio_name = win::default_audio_output_name();
        state.cached_muted = win::get_mute();
        state.last_poll = now;
    }

    let wifi_on = state.cached_wifi_on;
    let wifi_sub = if status.wifi_connected {
        if state.cached_wifi_name.is_empty() { "Connected".to_string() } else { state.cached_wifi_name.clone() }
    } else if wifi_on {
        "Not connected".to_string()
    } else {
        "Off".to_string()
    };

    let zone = tile(ui, row1[0], "Wi-Fi", &wifi_sub, Some(&icons.wifi), accent, 1.0, wifi_on, item_bg, bg);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleWifi),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Wifi)),
        TileZone::None => {}
    }

    let audio_sub = if state.cached_audio_name.is_empty() { "System output".to_string() } else { state.cached_audio_name.clone() };
    let muted = state.cached_muted;
    let audio_icon = if muted { &icons.audio_muted } else { &icons.audio };
    let zone = tile(ui, row1[1], "Audio", &audio_sub, Some(audio_icon), accent, 1.0, !muted, item_bg, bg);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleMute),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Audio)),
        TileZone::None => {}
    }

    let bt_on = state.cached_bt_on;
    let bt_sub = if status.bluetooth_connected {
        if state.cached_bt_name.is_empty() { "On".to_string() } else { state.cached_bt_name.clone() }
    } else if bt_on {
        "Not connected".to_string()
    } else {
        "Off".to_string()
    };
    let zone = tile(ui, row2[0], "Bluetooth", &bt_sub, Some(&icons.bt), accent, 1.0, bt_on, item_bg, bg);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleBluetooth),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Bluetooth)),
        TileZone::None => {}
    }

    let zone = tile(ui, row2[1], "Peace", if state.dnd { "On" } else { "Off" }, Some(&icons.moon), accent, 1.0, state.dnd, item_bg, bg);
    match zone {
        TileZone::Icon | TileZone::Text => actions.push(ControlAction::ToggleDnd),
        TileZone::None => {}
    }

    // Night Light row.
    let y2 = y1 + tile_h + gap;
    let zone = tile(
        ui,
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad, y2), egui::vec2(tile_w, tile_h)),
        "Night Light",
        if state.night_light { "On" } else { "Off" },
        Some(&icons.night),
        accent,
        1.0,
        state.night_light,
        item_bg,
        bg,
    );
    match zone {
        TileZone::Icon | TileZone::Text => actions.push(ControlAction::ToggleNightLight),
        TileZone::None => {}
    }

    // Power pill directly under Peace (right half of the Night Light row).
    let zone = tile(
        ui,
        egui::Rect::from_min_size(egui::pos2(rect.left() + pad + tile_w + gap, y2), egui::vec2(tile_w, tile_h)),
        "Power",
        "Power options",
        Some(&icons.power),
        accent,
        1.0,
        false,
        item_bg,
        bg,
    );
    if zone != TileZone::None {
        actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Power));
    }

    // Wallpapers + Themes row (under Night Light / Power).
    let y_pills = y2 + tile_h + gap;
    let pill_w = (content_w - gap) / 2.0;
    let pill_h = tile_h;
    let wp_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad, y_pills),
        egui::vec2(pill_w, pill_h),
    );
    let th_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad + pill_w + gap, y_pills),
        egui::vec2(pill_w, pill_h),
    );
    let wp_zone = tile(ui, wp_rect, "Wallpapers", "Pick wallpaper", Some(&icons.wallpaper), accent, 1.0, false, item_bg, bg);
    if wp_zone != TileZone::None {
        actions.push(ControlAction::OpenWallpapers);
    }
    let th_zone = tile(ui, th_rect, "Themes", "Switch theme", Some(&icons.theme), accent, 1.0, false, item_bg, bg);
    if th_zone != TileZone::None {
        actions.push(ControlAction::OpenThemes);
    }

    // Volume slider.
    let y3 = y_pills + pill_h + gap;
    let slider_h = 40.0;
    let mut vol = volume;
    let slider_response = slider_row(ui, y3, rect.left() + pad, content_w, slider_h, Some(&icons.audio), accent, vol, "volume", |v| vol = v, item_bg);
    if slider_response {
        win::set_volume(vol);
    }

    // Submenu area (appears when a tile's text is tapped).
    if state.submenu != ControlSubmenu::None {
        let y5 = y3 + slider_h + gap;
        let sub_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + pad, y5),
            egui::vec2(content_w, 220.0),
        );
        match state.submenu {
            ControlSubmenu::Audio => {
                if state.cached_audio_devices.is_empty() {
                    state.cached_audio_devices = win::list_audio_outputs()
                        .iter()
                        .map(|d| (d.name.clone(), d.id.clone()))
                        .collect();
                }
                let devices: Vec<(String, bool)> = state.cached_audio_devices.iter().map(|(n, _)| (n.clone(), false)).collect();
                if let Some(i) = submenu(ui, sub_rect, accent, "Audio output", devices, item_bg) {
                    if let Some((_, id)) = state.cached_audio_devices.get(i) {
                        actions.push(ControlAction::SetDefaultAudio(id.clone()));
                    }
                    state.cached_audio_devices.clear();
                }
            }
            ControlSubmenu::Wifi => {
                if state.cached_wifi_networks.is_empty() {
                    state.cached_wifi_networks = win::list_wifi_networks()
                        .iter()
                        .map(|n| n.ssid.clone())
                        .collect();
                }
                let nets: Vec<(String, bool)> = state.cached_wifi_networks.iter().map(|n| (n.clone(), false)).collect();
                if let Some(i) = submenu(ui, sub_rect, accent, "Wi-Fi networks", nets, item_bg) {
                    if let Some(ssid) = state.cached_wifi_networks.get(i) {
                        actions.push(ControlAction::ConnectWifi(ssid.clone()));
                    }
                    state.cached_wifi_networks.clear();
                }
            }
            ControlSubmenu::Bluetooth => {
                if state.cached_bt_devices.is_empty() {
                    state.cached_bt_devices = win::list_bluetooth_devices()
                        .iter()
                        .map(|(n, c)| (n.clone(), *c))
                        .collect();
                }
                let devs: Vec<(String, bool)> = state.cached_bt_devices.iter().map(|(n, c)| (n.clone(), *c)).collect();
                if submenu(ui, sub_rect, accent, "Bluetooth devices", devs, item_bg).is_some() {
                    state.cached_bt_devices.clear();
                }
            }
            ControlSubmenu::Power | ControlSubmenu::None => {}
        }
    } else {
        // Media card.
        if let Some((art, title, artist)) = media {
            let y5 = y3 + slider_h + gap;
            let card = egui::Rect::from_min_size(
                egui::pos2(rect.left() + pad, y5),
                egui::vec2(content_w, 96.0),
            );
            let corner = 22.0_f32;
            let cr = egui::CornerRadius::same(corner as u8);
            // Draw album cover clipped to card bounds.
            let clipped = ui.painter().with_clip_rect(card);
            let art_size = egui::vec2(card.width(), card.width());
            let art_rect = egui::Rect::from_center_size(card.center(), art_size);
            clipped.image(
                art.id(),
                art_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            drop(clipped);
            // Dark overlay for text legibility.
            ui.painter().rect_filled(card, cr, egui::Color32::from_rgba_premultiplied(0, 0, 0, 120));
            // Round the corners using the background color.
            let bg = Config::parse_color(&cfg.background);
            let bg = egui::Color32::from_rgb(bg.r(), bg.g(), bg.b());
            draw_corner_mask(ui, card, cr, bg);
            ui.painter().text(
                egui::pos2(card.left() + 14.0, card.center().y - 12.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(240, 240, 240),
            );
            ui.painter().text(
                egui::pos2(card.left() + 14.0, card.center().y + 10.0),
                egui::Align2::LEFT_CENTER,
                artist,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(160, 160, 160),
            );
        }
    }

    // Escape closes the submenu (return a close action, caller also handles).
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && state.submenu != ControlSubmenu::None {
        actions.push(ControlAction::CloseSubmenu);
    }

    actions
}

/// Draws corner patches to mask a rectangle's corners to a rounded rect shape.
fn draw_corner_mask(ui: &egui::Ui, rect: egui::Rect, cr: egui::CornerRadius, color: egui::Color32) {
    let r = cr.nw as f32;
    let steps = 8;
    let painter = ui.painter();
    // Top-left
    corner_mask(painter, rect.min, 1.0, 1.0, r, color, steps);
    // Top-right
    corner_mask(painter, egui::pos2(rect.right(), rect.top()), -1.0, 1.0, r, color, steps);
    // Bottom-left
    corner_mask(painter, egui::pos2(rect.left(), rect.bottom()), 1.0, -1.0, r, color, steps);
    // Bottom-right
    corner_mask(painter, rect.max, -1.0, -1.0, r, color, steps);
}

fn corner_mask(
    painter: &egui::Painter,
    corner: egui::Pos2,
    dx: f32,
    dy: f32,
    r: f32,
    bg: egui::Color32,
    steps: usize,
) {
    let mut pts = [egui::Pos2::ZERO; 12];
    let count = (steps + 2).min(pts.len());
    pts[0] = corner;
    for i in 0..=steps.min(count - 2) {
        let t = std::f32::consts::FRAC_PI_2 * (i as f32 / steps as f32);
        pts[i + 1] = corner + egui::vec2(dx * r * t.cos(), dy * r * t.sin());
    }
    pts[count - 1] = corner;
    painter.add(egui::epaint::PathShape::convex_polygon(
        pts[..count].to_vec(), bg, egui::Stroke::NONE,
    ));
}

/// Truncates `text` with an ellipsis so its rendered width fits `max_w`.
/// Uses a thread-local cache to avoid repeated binary searches for the same input.
fn truncate_to_fit(painter: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> String {
    use std::collections::HashMap;
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<HashMap<(String, u32), String>> = RefCell::new(HashMap::new());
    }
    let key = (text.to_string(), max_w.to_bits());
    if let Some(cached) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return cached;
    }
    let result = if max_w <= 0.0 {
        String::new()
    } else {
        let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
        if measure(text) <= max_w {
            text.to_string()
        } else {
            let ellipsis_w = measure("…");
            let mut lo = 0usize;
            let mut hi = text.chars().count();
            let mut best = 0usize;
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                let prefix: String = text.chars().take(mid).collect();
                if measure(&prefix) + ellipsis_w <= max_w {
                    best = mid;
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let mut out: String = text.chars().take(best).collect();
            out.push('…');
            out
        }
    };
    CACHE.with(|c| c.borrow_mut().insert(key, result.clone()));
    result
}
