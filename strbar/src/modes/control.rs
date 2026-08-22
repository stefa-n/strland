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
    pub sun: egui::TextureHandle,
    pub power: egui::TextureHandle,
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
}

/// Mutable state for the control-center toggles (persisted across frames).
#[derive(Clone)]
pub struct ControlCenterState {
    pub dnd: bool,
    pub night_light: bool,
    pub submenu: ControlSubmenu,
}

impl Default for ControlCenterState {
    fn default() -> Self {
        Self { dnd: false, night_light: false, submenu: ControlSubmenu::None }
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
) -> TileZone {
    let response = ui.interact(rect, ui.make_persistent_id(("cctile", label)), egui::Sense::click());
    let hovered = response.hovered() || on;

    let (bg, fg) = if on {
        (
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), 255),
            egui::Color32::from_rgb(18, 18, 18),
        )
    } else {
        let lift = if hovered { 14.0 } else { 0.0 };
        (
            egui::Color32::from_rgb(
                (24.0 + lift) as u8,
                (24.0 + lift) as u8,
                (26.0 + lift) as u8,
            ),
            egui::Color32::from_rgb(220, 220, 220),
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
    ui.painter().text(
        egui::pos2(rect.left() + 50.0, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        sublabel,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
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
) {
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
    ui.painter().rect_filled(rect, egui::CornerRadius::same((height / 2.0) as u8), egui::Color32::from_rgb(34, 34, 36));

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
            egui::Color32::from_rgb(235, 235, 235),
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
    if let Some(pos) = response.interact_pointer_pos() {
        let frac = ((pos.x - slider_x) / slider_w).clamp(0.0, 1.0);
        on_change(frac);
    }
}

/// Draws a submenu as a column of simple list rows.
fn submenu(
    ui: &egui::Ui,
    rect: egui::Rect,
    accent: egui::Color32,
    title: &str,
    rows: Vec<(String, bool)>, // (label, is_current)
) -> Option<usize> {
    let bg = egui::Color32::from_rgb(28, 28, 30);
    ui.painter().rect_filled(rect, egui::CornerRadius::same(22), bg);

    let pad = 14.0;
    ui.painter().text(
        egui::pos2(rect.left() + pad, rect.top() + pad),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
        egui::Color32::from_rgb(230, 230, 230),
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
            egui::Color32::from_rgb(230, 230, 230),
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
) -> Vec<ControlAction> {
    let mut actions: Vec<ControlAction> = Vec::new();
    let bg = Config::parse_color(&cfg.background);
    let bg = egui::Color32::from_rgb(bg.r(), bg.g(), bg.b());
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
    let wifi_name = win::current_wifi_ssid();
    let wifi_sub = if status.wifi_connected {
        if wifi_name.is_empty() { "Connected".to_string() } else { wifi_name }
    } else {
        "Off".to_string()
    };

    let zone = tile(ui, row1[0], "Wi-Fi", &wifi_sub, Some(&icons.wifi), accent, 1.0, status.wifi_connected);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleWifi),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Wifi)),
        TileZone::None => {}
    }

    let audio_name = win::default_audio_output_name();
    let audio_sub = if audio_name.is_empty() { "System output".to_string() } else { audio_name };
    let muted = win::get_mute();
    let audio_icon = if muted { &icons.audio_muted } else { &icons.audio };
    let zone = tile(ui, row1[1], "Audio", &audio_sub, Some(audio_icon), accent, 1.0, !muted);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleMute),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Audio)),
        TileZone::None => {}
    }

    let bt_name = win::current_bluetooth_name();
    let bt_sub = if status.bluetooth_connected {
        if bt_name.is_empty() { "On".to_string() } else { bt_name }
    } else {
        "Off".to_string()
    };
    let zone = tile(ui, row2[0], "Bluetooth", &bt_sub, Some(&icons.bt), accent, 1.0, status.bluetooth_connected);
    match zone {
        TileZone::Icon => actions.push(ControlAction::ToggleBluetooth),
        TileZone::Text => actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Bluetooth)),
        TileZone::None => {}
    }

    let zone = tile(ui, row2[1], "Peace", if state.dnd { "On" } else { "Off" }, Some(&icons.moon), accent, 1.0, state.dnd);
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
    );
    if zone != TileZone::None {
        actions.push(ControlAction::OpenSubmenu(ControlSubmenu::Power));
    }

    // Volume + brightness sliders.
    let y3 = y2 + tile_h + gap;
    let slider_h = 40.0;
    let mut vol = volume;
    slider_row(ui, y3, rect.left() + pad, content_w, slider_h, Some(&icons.audio), accent, vol, "volume", |v| vol = v);
    win::set_volume(vol);

    let y4 = y3 + slider_h + gap;
    let mut bright = 0.8f32;
    slider_row(ui, y4, rect.left() + pad, content_w, slider_h, Some(&icons.sun), accent, bright, "brightness", |v| bright = v);

    // Submenu area (appears when a tile's text is tapped).
    if state.submenu != ControlSubmenu::None {
        let y5 = y4 + slider_h + gap;
        let sub_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + pad, y5),
            egui::vec2(content_w, 220.0),
        );
        match state.submenu {
            ControlSubmenu::Audio => {
                let devices: Vec<(String, bool)> = win::list_audio_outputs()
                    .iter()
                    .map(|d| (d.name.clone(), false))
                    .collect();
                if let Some(i) = submenu(ui, sub_rect, accent, "Audio output", devices) {
                    if let Some(dev) = win::list_audio_outputs().get(i) {
                        actions.push(ControlAction::SetDefaultAudio(dev.id.clone()));
                    }
                }
            }
            ControlSubmenu::Wifi => {
                let nets: Vec<(String, bool)> = win::list_wifi_networks()
                    .iter()
                    .map(|n| (n.ssid.clone(), false))
                    .collect();
                if let Some(i) = submenu(ui, sub_rect, accent, "Wi-Fi networks", nets) {
                    if let Some(net) = win::list_wifi_networks().get(i) {
                        actions.push(ControlAction::ConnectWifi(net.ssid.clone()));
                    }
                }
            }
            ControlSubmenu::Bluetooth => {
                let devs: Vec<(String, bool)> = win::list_bluetooth_devices()
                    .iter()
                    .map(|(n, c)| (n.clone(), *c))
                    .collect();
                let _ = submenu(ui, sub_rect, accent, "Bluetooth devices", devs);
            }
            ControlSubmenu::Power | ControlSubmenu::None => {}
        }
    } else {
        // Media card.
        if let Some((art, title, artist)) = media {
            let y5 = y4 + slider_h + gap;
            let card = egui::Rect::from_min_size(
                egui::pos2(rect.left() + pad, y5),
                egui::vec2(content_w, 96.0),
            );
            ui.painter().rect_filled(card, egui::CornerRadius::same(22), egui::Color32::from_rgb(30, 30, 32));
            let art_side = 72.0;
            let art_rect = egui::Rect::from_min_size(
                egui::pos2(card.left() + 14.0, card.center().y - art_side / 2.0),
                egui::vec2(art_side, art_side),
            );
            ui.painter().image(
                art.id(),
                art_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.painter().text(
                egui::pos2(art_rect.right() + 14.0, card.center().y - 16.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                egui::Color32::from_rgb(240, 240, 240),
            );
            ui.painter().text(
                egui::pos2(art_rect.right() + 14.0, card.center().y + 6.0),
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
